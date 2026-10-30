use std::fs::File;
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::identity::{Owner, group_members, group_unchanged, owner};

/// Starts `command` detached in its own process group, with stdout and stderr appended to
/// `output`. The wrapper writes the exit code to `exit` so a later process can read it.
pub fn spawn(command: &str, cwd: &Path, output: &Path, exit: &Path) -> io::Result<u32> {
    let log = private_append(output)?;
    let mut cmd = wrapper(command, exit);
    cmd.current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    let child = Command::from(cmd).spawn()?;
    child
        .id()
        .ok_or_else(|| io::Error::other("process exited before it had a pid"))
}

#[cfg(unix)]
fn private_append(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let file = File::options()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

#[cfg(windows)]
fn private_append(path: &Path) -> io::Result<File> {
    File::options().create(true).append(true).open(path)
}

#[cfg(unix)]
fn wrapper(command: &str, exit: &Path) -> std::process::Command {
    use std::os::unix::process::CommandExt;
    const SCRIPT: &str =
        r#""$3" -c "$1"; c=$?; umask 077; printf '%s' "$c" > "$2.tmp"; mv -f "$2.tmp" "$2""#;
    let shell = if Path::new("/bin/bash").exists() {
        "/bin/bash"
    } else {
        "/bin/sh"
    };
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(SCRIPT)
        .arg("sh")
        .arg(command)
        .arg(exit)
        .arg(shell)
        .process_group(0);
    cmd
}

#[cfg(windows)]
fn wrapper(command: &str, exit: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("cmd");
    let script = format!("{command} & call echo %^ERRORLEVEL%>{}", exit.display());
    cmd.arg("/C").arg(script);
    cmd
}

/// True while any process of the group led by `pid` exists (unix); the process itself on Windows.
pub async fn group_alive(pid: u32) -> bool {
    #[cfg(unix)]
    let target = format!("-{pid}");
    #[cfg(windows)]
    let target = pid.to_string();
    alive_impl(&target).await.unwrap_or(false)
}

#[cfg(unix)]
async fn alive_impl(target: &str) -> io::Result<bool> {
    let status = quiet(Command::new("kill").args(["-0", "--", target]))
        .status()
        .await?;
    Ok(status.success())
}

#[cfg(windows)]
async fn alive_impl(pid: &str) -> io::Result<bool> {
    let filter = format!("PID eq {pid}");
    let out = Command::new("tasklist")
        .args(["/FI", &filter, "/NH"])
        .output()
        .await?;
    Ok(String::from_utf8_lossy(&out.stdout).contains(&pid.to_string()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Term,
    Kill,
}

/// Sends `signal` to the whole process group (unix) or process tree (Windows) of `pid`.
/// Fails when the signal command fails; an already empty group also fails.
pub async fn signal_group(pid: u32, signal: Signal) -> io::Result<()> {
    signal_impl(pid, signal).await
}

fn checked(status: std::process::ExitStatus, what: &str) -> io::Result<()> {
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{what} exited with {status}")))
    }
}

#[cfg(unix)]
async fn signal_impl(pid: u32, signal: Signal) -> io::Result<()> {
    let name = match signal {
        Signal::Term => "TERM",
        Signal::Kill => "KILL",
    };
    let group = format!("-{pid}");
    let status = quiet(Command::new("kill").args(["-s", name, "--", &group]))
        .status()
        .await?;
    checked(status, "kill")
}

#[cfg(windows)]
async fn signal_impl(pid: u32, _signal: Signal) -> io::Result<()> {
    let status = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    checked(status, "taskkill")
}

#[cfg(unix)]
fn quiet(cmd: &mut Command) -> &mut Command {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
}

pub fn unverified_message(pid: u32) -> String {
    format!("pid {pid} is not the task's process (reused or unreadable); no signal sent")
}

/// TERM to the process group, KILL to whatever is left after `grace`; fails when processes remain.
/// TERM needs `pid` to match `identity`; KILL also accepts the group members seen before TERM.
pub async fn terminate(pid: u32, identity: Option<&str>, grace: Duration) -> Result<(), String> {
    if owner(pid, identity) != Owner::Same {
        return Err(unverified_message(pid));
    }
    let members = group_members(pid).unwrap_or_default();
    if let Err(err) = signal_group(pid, Signal::Term).await {
        if group_alive(pid).await {
            return Err(format!("cannot stop process group {pid}: {err}"));
        }
        return Ok(());
    }
    let deadline = tokio::time::Instant::now() + grace;
    while tokio::time::Instant::now() < deadline {
        if !group_alive(pid).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let ours = match owner(pid, identity) {
        Owner::Same => true,
        Owner::Gone => group_unchanged(pid, &members),
        Owner::Other => false,
    };
    if !ours {
        return Err(unverified_message(pid));
    }
    let killed = signal_group(pid, Signal::Kill).await;
    for _ in 0..10 {
        if !group_alive(pid).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(match killed {
        Err(err) => format!("cannot kill process group {pid}: {err}"),
        Ok(()) => format!("process group {pid} still runs after SIGKILL"),
    })
}
