use std::process::Stdio;
use std::time::Duration;

use pi_ext_sdk::{CancellationToken, Error, Result};
use serde_json::Value;
use tokio::process::Command;

use super::Cmd;

#[derive(Debug, Clone)]
pub struct Output {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.status == 0
    }
}

/// Runs `cmd` with a time limit; the child is killed on cancel or timeout.
pub async fn run(cmd: &Cmd, timeout: Duration, cancel: &CancellationToken) -> Result<Output> {
    let child = Command::new(&cmd.program)
        .args(&cmd.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| Error::msg(format!("Could not start {}: {err}", cmd.program)))?;
    let output = tokio::select! {
        output = tokio::time::timeout(timeout, child.wait_with_output()) => output,
        () = cancel.cancelled() => return Err(Error::msg("Operation aborted.")),
    };
    let output = output
        .map_err(|_| {
            Error::msg(format!(
                "{} timed out after {}s",
                cmd.program,
                timeout.as_secs()
            ))
        })?
        .map_err(|err| Error::msg(format!("{} failed: {err}", cmd.program)))?;
    Ok(Output {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

pub async fn run_ok(cmd: &Cmd, timeout: Duration, cancel: &CancellationToken) -> Result<String> {
    let output = run(cmd, timeout, cancel).await?;
    if output.ok() {
        return Ok(output.stdout);
    }
    let detail = if output.stderr.trim().is_empty() {
        &output.stdout
    } else {
        &output.stderr
    };
    Err(Error::msg(format!(
        "{} exited with {}: {}",
        cmd.program,
        output.status,
        detail.trim()
    )))
}

/// Parses the helper envelope `{ ok, result | error }`.
pub fn helper_result(stdout: &str) -> Result<Value> {
    let line = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    let envelope: Value = serde_json::from_str(line)
        .map_err(|err| Error::msg(format!("Helper returned invalid JSON: {err}")))?;
    if envelope["ok"] == true {
        return Ok(envelope["result"].clone());
    }
    let message = envelope["error"].as_str().unwrap_or("Helper failed");
    Err(Error::msg(message.to_owned()))
}

pub fn which(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(program).is_file())
}
