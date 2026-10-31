//! Runs one child pi process and streams its events into the run registry.

use std::process::Stdio;
use std::time::Duration;

use pi_ext_sdk::CancellationToken;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::command::ChildSpec;
use crate::runs::{Runs, Status};
use crate::stream::Effect;

const STDERR_CAP: usize = 16 * 1024;
const KILL_GRACE: Duration = Duration::from_secs(1);
/// How long shutdown waits for cancelled runs: above `KILL_GRACE`, below the SDK's 1.5 s limit.
pub const SHUTDOWN_WAIT: Duration = Duration::from_millis(1400);

pub async fn execute(
    spec: &ChildSpec,
    runs: &Runs,
    id: &str,
    cancel: &CancellationToken,
    on_effect: &mut (dyn FnMut(Effect) + Send),
) -> Status {
    runs.update(id, |run| run.model = spec.model.clone());
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            let message = format!("cannot start {}: {err}", spec.program.display());
            runs.update(id, |run| run.progress.error = Some(message));
            return Status::Failed;
        }
    };
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr_task = child.stderr.take().map(|stderr| {
        let runs = runs.clone();
        let id = id.to_owned();
        tokio::spawn(async move { collect_stderr(stderr, &runs, &id).await })
    });
    let mut lines = BufReader::new(stdout).lines();
    let cancelled = loop {
        tokio::select! {
            _ = cancel.cancelled() => break true,
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    let effect = runs.update(id, |run| run.progress.apply_line(&line));
                    if let Some(effect) = effect.filter(|e| *e != Effect::None) {
                        on_effect(effect);
                    }
                }
                _ => break false,
            },
        }
    };
    if cancelled {
        stop(&mut child).await;
        if let Some(task) = stderr_task {
            task.abort();
        }
        return Status::Cancelled;
    }
    let exit = tokio::select! {
        _ = cancel.cancelled() => {
            stop(&mut child).await;
            return Status::Cancelled;
        }
        exit = child.wait() => exit,
    };
    if let Some(task) = stderr_task {
        let _ = tokio::time::timeout(Duration::from_secs(1), task).await;
    }
    let code = exit.ok().and_then(|status| status.code());
    let failed = runs
        .update(id, |run| {
            run.exit_code = code;
            run.progress.error.is_some()
        })
        .unwrap_or(true);
    if failed || code != Some(0) {
        Status::Failed
    } else {
        Status::Completed
    }
}

async fn collect_stderr(stderr: impl AsyncReadExt + Unpin, runs: &Runs, id: &str) {
    let mut reader = BufReader::new(stderr);
    let mut buf = [0u8; 4096];
    while let Ok(n) = reader.read(&mut buf).await {
        if n == 0 {
            break;
        }
        let chunk = String::from_utf8_lossy(&buf[..n]).into_owned();
        runs.update(id, |run| {
            run.stderr.push_str(&chunk);
            if run.stderr.len() > STDERR_CAP {
                let mut cut = run.stderr.len() - STDERR_CAP;
                while !run.stderr.is_char_boundary(cut) {
                    cut += 1;
                }
                run.stderr.drain(..cut);
            }
        });
    }
}

/// SIGTERM to the child's process group, then SIGKILL after `KILL_GRACE` (inside the SDK cancel window).
async fn stop(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let _ = group_signal_command("-TERM", pid).status();
    }
    if tokio::time::timeout(KILL_GRACE, child.wait())
        .await
        .is_err()
    {
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            let _ = group_signal_command("-KILL", pid).status();
        }
        let _ = child.kill().await;
    }
}

#[cfg(unix)]
fn group_signal_command(signal: &str, pid: u32) -> std::process::Command {
    let mut command = std::process::Command::new("kill");
    command
        .args([signal, "--", &format!("-{pid}")])
        .stderr(Stdio::null());
    command
}

#[cfg(all(test, unix))]
mod tests {
    use super::group_signal_command;

    #[test]
    fn process_group_signals_terminate_option_parsing() {
        for signal in ["-TERM", "-KILL"] {
            for pid in [42, 1_439_999, 2_147_483_000, i32::MAX as u32] {
                let command = group_signal_command(signal, pid);
                assert_eq!(command.get_program(), "kill");
                let args: Vec<_> = command.get_args().collect();
                assert_eq!(args, [signal, "--", &format!("-{pid}")]);
            }
        }
    }
}
