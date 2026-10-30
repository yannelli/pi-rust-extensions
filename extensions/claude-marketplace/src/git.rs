use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

const TIMEOUT: Duration = Duration::from_secs(300);

fn run(args: &[&str], dir: Option<&Path>) -> Result<String> {
    let mut command = Command::new("git");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    if let Some(dir) = dir {
        command.arg("-C").arg(dir);
    }
    let mut child = command
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("cannot run git: is it installed and on PATH?")?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_thread = std::thread::spawn(move || read_all(stdout));
    let err_thread = std::thread::spawn(move || read_all(stderr));
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() > deadline {
            kill_group(child.id());
            let _ = child.kill();
            let _ = child.wait();
            bail!("git {} timed out after {TIMEOUT:?}", args.join(" "));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    if !status.success() {
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(stdout)
}

fn kill_group(pid: u32) {
    #[cfg(unix)]
    let mut command = Command::new("kill");
    #[cfg(unix)]
    command.args(["-KILL", "--", &format!("-{pid}")]);
    #[cfg(windows)]
    let mut command = Command::new("taskkill");
    #[cfg(windows)]
    command.args(["/T", "/F", "/PID", &pid.to_string()]);
    let _ = command.stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// A commit id from a marketplace file: 4 to 64 hex digits.
pub fn check_sha(sha: &str) -> Result<()> {
    if !(4..=64).contains(&sha.len()) || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("invalid commit sha \"{sha}\": expected 4 to 64 hex digits");
    }
    Ok(())
}

fn read_all(stream: Option<impl Read>) -> String {
    let mut text = String::new();
    if let Some(mut stream) = stream {
        let _ = stream.read_to_string(&mut text);
    }
    text
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("path is not UTF-8: {}", path.display()))
}

/// Clones `url` into `dest`. With `sha` the clone is full so the commit can be checked out.
pub fn clone(url: &str, dest: &Path, git_ref: Option<&str>, sha: Option<&str>) -> Result<()> {
    if let Some(sha) = sha {
        check_sha(sha)?;
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dest_str = path_str(dest)?;
    let mut args = vec!["clone", "--quiet"];
    if sha.is_none() {
        args.extend(["--depth", "1"]);
    }
    let branch = git_ref.map(|git_ref| format!("--branch={git_ref}"));
    if let Some(branch) = &branch {
        args.push(branch);
    }
    args.extend(["--", url, dest_str]);
    run(&args, None)?;
    if let Some(sha) = sha {
        run(
            &["checkout", "--quiet", "--detach", "--end-of-options", sha],
            Some(dest),
        )?;
    }
    Ok(())
}

/// Brings a clone up to date: `pull --ff-only`, else fetch and hard reset.
pub fn update(dir: &Path) -> Result<()> {
    if run(&["pull", "--ff-only", "--quiet"], Some(dir)).is_ok() {
        return Ok(());
    }
    run(
        &["fetch", "--quiet", "--depth", "1", "origin", "HEAD"],
        Some(dir),
    )?;
    run(&["reset", "--quiet", "--hard", "FETCH_HEAD"], Some(dir))?;
    Ok(())
}
