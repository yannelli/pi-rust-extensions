//! Runs the `plutil` and `reg` policy readers with the limits that Claude Code 2.1.329 sets.

use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub timeout: Duration,
    pub max_output: usize,
}

pub const CLAUDE_LIMITS: Limits = Limits {
    timeout: Duration::from_secs(5),
    max_output: 2 * 1024 * 1024,
};

#[derive(Debug)]
pub struct Finished {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Runs `command` to the end. The error names the limit it broke, or why it did not start; the
/// command is killed when it breaks a limit.
pub fn run(command: &mut Command, limits: Limits) -> Result<Finished, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| err.to_string())?;
    let stdout = read_capped(child.stdout.take(), limits.max_output);
    let stderr = read_capped(child.stderr.take(), limits.max_output);
    let deadline = Instant::now() + limits.timeout;
    let mut out: Option<Vec<u8>> = None;
    let broke = loop {
        if out.is_none() {
            out = stdout.try_recv().ok();
        }
        if out
            .as_ref()
            .is_some_and(|out| out.len() > limits.max_output)
        {
            break format!("its output is larger than {} bytes", limits.max_output);
        }
        if out.is_some()
            && let Ok(Some(status)) = child.try_wait()
        {
            let wait = deadline.saturating_duration_since(Instant::now());
            return Ok(Finished {
                status,
                stdout: out.unwrap_or_default(),
                stderr: stderr.recv_timeout(wait).unwrap_or_default(),
            });
        }
        if Instant::now() >= deadline {
            break format!("it did not finish within {:?}", limits.timeout);
        }
        thread::sleep(Duration::from_millis(5));
    };
    let _ = child.kill();
    let _ = child.wait();
    Err(broke)
}

/// Reads `pipe` on its own thread up to one byte past `cap`, so an overflow shows.
fn read_capped(pipe: Option<impl Read + Send + 'static>, cap: usize) -> Receiver<Vec<u8>> {
    let (send, receive) = channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(pipe) = pipe {
            let _ = pipe.take(cap as u64 + 1).read_to_end(&mut bytes);
        }
        let _ = send.send(bytes);
    });
    receive
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const SHORT: Limits = Limits {
        timeout: Duration::from_millis(300),
        max_output: 16,
    };

    fn sh(script: &str) -> Result<Finished, String> {
        run(Command::new("/bin/sh").args(["-c", script]), SHORT)
    }

    #[test]
    fn a_finished_command_gives_its_status_and_output() {
        let done = sh("printf ok; printf no >&2; exit 3").unwrap();
        assert_eq!(done.status.code(), Some(3));
        assert_eq!(done.stdout, b"ok");
        assert_eq!(done.stderr, b"no");
    }

    #[test]
    fn a_slow_command_is_killed_at_the_timeout() {
        let started = Instant::now();
        let err = sh("sleep 5").unwrap_err();
        assert!(err.contains("did not finish within 300ms"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn output_past_the_cap_stops_the_command() {
        let started = Instant::now();
        let err = sh("while :; do echo 0123456789; done").unwrap_err();
        assert!(err.contains("larger than 16 bytes"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(sh("printf 0123456789abcdef").is_ok());
    }
}
