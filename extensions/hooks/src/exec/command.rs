use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

use crate::output::RawRun;

pub const STDOUT_CAP: usize = 1024 * 1024;
pub const STDERR_CAP: usize = 64 * 1024;
#[cfg(unix)]
const KILL_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub command: String,
    /// `Some` runs `command` with these args and no shell.
    pub args: Option<Vec<String>>,
    pub stdin: String,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
}

pub async fn run_command(spec: CommandSpec) -> RawRun {
    let mut command = match &spec.args {
        Some(args) => {
            let mut command = Command::new(&spec.command);
            command.args(args);
            command
        }
        None => shell_command(&spec.command),
    };
    if spec.cwd.is_dir() {
        command.current_dir(&spec.cwd);
    }
    command
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut guard = match command.spawn() {
        Ok(child) => GroupGuard(Some(child)),
        Err(err) => return RawRun::failed(format!("cannot start: {err}")),
    };
    let Some(child) = guard.0.as_mut() else {
        return RawRun::failed("child vanished");
    };
    if let Some(mut stdin) = child.stdin.take() {
        let input = spec.stdin.into_bytes();
        tokio::spawn(async move {
            let _ = stdin.write_all(&input).await;
        });
    }
    let stdout = child
        .stdout
        .take()
        .map(|out| tokio::spawn(read_capped(out, STDOUT_CAP)));
    let stderr = child
        .stderr
        .take()
        .map(|err| tokio::spawn(read_capped(err, STDERR_CAP)));
    let (code, error) = match tokio::time::timeout(spec.timeout, child.wait()).await {
        Ok(Ok(status)) => (status.code(), None),
        Ok(Err(err)) => (None, Some(format!("wait failed: {err}"))),
        Err(_) => {
            terminate(child).await;
            (
                None,
                Some(format!("timed out after {}s", spec.timeout.as_secs_f64())),
            )
        }
    };
    guard.0 = None;
    let collect = |task: Option<tokio::task::JoinHandle<String>>| async move {
        let task = task?;
        let abort = task.abort_handle();
        match tokio::time::timeout(Duration::from_secs(1), task).await {
            Ok(Ok(text)) => Some(text),
            _ => {
                abort.abort();
                None
            }
        }
    };
    let stdout = collect(stdout).await.unwrap_or_default();
    let stderr = collect(stderr).await.unwrap_or_default();
    RawRun {
        code,
        stdout,
        stderr,
        error,
    }
}

/// Stops the process group when the hook future is dropped (cancelled) before the child exits.
struct GroupGuard(Option<Child>);

impl Drop for GroupGuard {
    fn drop(&mut self) {
        let Some(mut child) = self.0.take() else {
            return;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move { terminate(&mut child).await });
        }
    }
}

#[cfg(unix)]
fn shell_command(line: &str) -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(line);
    command
}

#[cfg(windows)]
fn shell_command(line: &str) -> Command {
    let mut command = Command::new("cmd");
    command
        .args(["/d", "/s", "/c"])
        .raw_arg(format!("\"{line}\""));
    command
}

/// SIGTERM to the process group, then SIGKILL after the grace period.
#[cfg(unix)]
async fn terminate(child: &mut Child) {
    let Some(pid) = child.id() else {
        return;
    };
    signal_group("-TERM", pid).await;
    if tokio::time::timeout(KILL_GRACE, child.wait())
        .await
        .is_err()
    {
        signal_group("-KILL", pid).await;
        let _ = child.kill().await;
    }
}

/// Windows has no signals: `taskkill /T /F` ends the process tree at once.
#[cfg(windows)]
async fn terminate(child: &mut Child) {
    if let Some(pid) = child.id() {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    let _ = child.kill().await;
}

#[cfg(unix)]
async fn signal_group(signal: &str, pid: u32) {
    let _ = Command::new("kill")
        .args([signal, "--", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

/// Keeps the first `cap` bytes and drains the rest so the child does not block on a full pipe.
async fn read_capped(mut reader: impl AsyncRead + Unpin, cap: usize) -> String {
    let mut kept = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = cap.saturating_sub(kept.len());
                kept.extend_from_slice(&buf[..n.min(room)]);
            }
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn spec(command: &str, timeout: u64) -> CommandSpec {
        CommandSpec {
            command: command.into(),
            args: None,
            stdin: r#"{"tool_name":"Bash"}"#.into(),
            cwd: std::env::temp_dir(),
            env: vec![("HOOK_TEST".into(), "v".into())],
            timeout: Duration::from_secs(timeout),
        }
    }

    #[tokio::test]
    async fn reads_stdin_and_exit_code() {
        let run = run_command(spec("cat; echo \" $HOOK_TEST\"; echo err >&2; exit 2", 10)).await;
        assert_eq!(run.code, Some(2));
        assert_eq!(run.stdout.trim(), r#"{"tool_name":"Bash"} v"#);
        assert_eq!(run.stderr.trim(), "err");
    }

    #[tokio::test]
    async fn exec_form_skips_the_shell() {
        let mut s = spec("printf", 10);
        s.args = Some(vec!["%s-%s".into(), "$HOME".into(), "b".into()]);
        assert_eq!(run_command(s).await.stdout, "$HOME-b");
    }

    #[tokio::test]
    async fn timeout_kills_the_group() {
        let started = std::time::Instant::now();
        let run = run_command(spec("sleep 30 & sleep 30", 1)).await;
        assert_eq!(run.code, None);
        assert!(run.error.unwrap().starts_with("timed out"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn cancel_kills_the_group() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let script = format!("sleep 30 & echo $! > {}; wait", pid_file.display());
        let task = tokio::spawn(run_command(spec(&script, 60)));
        let pid = loop {
            if let Ok(text) = std::fs::read_to_string(&pid_file)
                && text.ends_with('\n')
            {
                break text.trim().to_owned();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        task.abort();
        let _ = task.await;
        let alive = || async {
            Command::new("kill")
                .args(["-0", &pid])
                .stderr(Stdio::null())
                .status()
                .await
                .is_ok_and(|s| s.success())
        };
        for _ in 0..100 {
            if !alive().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("grandchild {pid} still runs after cancel");
    }

    #[tokio::test]
    async fn caps_stdout() {
        let run = run_command(spec("head -c 2000000 /dev/zero | tr '\\0' a", 20)).await;
        assert_eq!(run.code, Some(0));
        assert_eq!(run.stdout.len(), STDOUT_CAP);
    }

    #[tokio::test]
    async fn missing_program_fails() {
        let mut s = spec("/no/such/program", 5);
        s.args = Some(vec![]);
        assert!(
            run_command(s)
                .await
                .error
                .unwrap()
                .starts_with("cannot start")
        );
    }
}
