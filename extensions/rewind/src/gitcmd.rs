use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

use pi_ext_sdk::{Error, Result, bail};
use tokio::process::Command;

const GIT_TIMEOUT: Duration = Duration::from_secs(120);
const SCRUBBED_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_COMMON_DIR",
];

pub fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| Error::msg(format!("path is not UTF-8: {}", path.display())))
}

pub async fn run_git(cwd: Option<&Path>, args: &[&str]) -> Result<String> {
    let output = git_output(cwd, args).await?;
    if !output.status.success() {
        bail!("{}", failure(args, &output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn failure(args: &[&str], output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("git {} failed: {}", args.join(" "), stderr.trim())
}

/// Runs git and returns its output whatever the exit status.
pub async fn git_output(cwd: Option<&Path>, args: &[&str]) -> Result<Output> {
    let mut command = Command::new("git");
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true);
    for key in SCRUBBED_ENV {
        command.env_remove(key);
    }
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = tokio::time::timeout(GIT_TIMEOUT, command.output())
        .await
        .map_err(|_| Error::msg(format!("git {} timed out", args.join(" "))))??;
    Ok(output)
}
