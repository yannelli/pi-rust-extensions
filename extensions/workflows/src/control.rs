//! Files that connect pi sessions with runners. A session sends a runner commands as files in
//! `<run dir>/commands`, and each pi keeps `<root>/sessions/<pid>` fresh while it runs, so a
//! foreground run can stop when its pi is gone.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::runs::now_ms;
use crate::store::write_atomic;

pub const COMMANDS_DIR: &str = "commands";
/// Messages from the user to the agent that runs now; the agent reads them as steering.
pub const INBOX_DIR: &str = "inbox";
/// How often a pi refreshes its liveness file.
pub const TOUCH_EVERY: Duration = Duration::from_secs(2);
/// A liveness file older than this means that pi is gone.
const ALIVE_GRACE_MS: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Command {
    Stop,
    /// pi quits: the run stops as interrupted, so it can resume later.
    Interrupt,
    /// End the current wait at once.
    Skip,
    Approve {
        decision: Decision,
    },
    Mode {
        background: bool,
        owner: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Run,
    Skip,
    Stop,
}

impl Decision {
    pub fn parse(text: &str) -> Option<Decision> {
        match text.trim().to_ascii_lowercase().as_str() {
            "" | "run" | "yes" | "approve" => Some(Decision::Run),
            "skip" => Some(Decision::Skip),
            "stop" => Some(Decision::Stop),
            _ => None,
        }
    }
}

static NEXT: AtomicU64 = AtomicU64::new(0);

pub fn send(run_dir: &Path, command: &Command) -> std::io::Result<()> {
    let text = serde_json::to_string(command).map_err(std::io::Error::other)?;
    queue(&run_dir.join(COMMANDS_DIR), &text)
}

/// The waiting commands, oldest first; each file is removed as it is read.
pub fn take(run_dir: &Path) -> Vec<Command> {
    take_queued(&run_dir.join(COMMANDS_DIR))
        .iter()
        .filter_map(|text| serde_json::from_str(text).ok())
        .collect()
}

pub fn steer(inbox: &Path, message: &str) -> std::io::Result<()> {
    queue(
        inbox,
        &serde_json::to_string(message).map_err(std::io::Error::other)?,
    )
}

pub fn take_steering(inbox: &Path) -> Vec<String> {
    take_queued(inbox)
        .iter()
        .filter_map(|text| serde_json::from_str(text).ok())
        .collect()
}

fn queue(dir: &Path, text: &str) -> std::io::Result<()> {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let name = format!("{:013}-{}-{n}.json", now_ms(), std::process::id());
    write_atomic(&dir.join(name), text)
}

fn take_queued(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok();
            let _ = std::fs::remove_file(&path);
            text
        })
        .collect()
}

/// The process id of the pi this extension runs in; it stays the same across `/reload`.
pub fn pi_pid() -> u32 {
    #[cfg(unix)]
    {
        std::os::unix::process::parent_id()
    }
    #[cfg(not(unix))]
    {
        std::process::id()
    }
}

fn alive_file(root: &Path, pid: u32) -> PathBuf {
    root.join("sessions").join(pid.to_string())
}

pub fn touch(root: &Path, pid: u32) {
    let _ = write_atomic(&alive_file(root, pid), &now_ms().to_string());
}

pub fn forget(root: &Path, pid: u32) {
    let _ = std::fs::remove_file(alive_file(root, pid));
}

/// Whether the pi with this process id refreshed its liveness file recently.
pub fn alive(root: &Path, pid: u32) -> bool {
    let Ok(text) = std::fs::read_to_string(alive_file(root, pid)) else {
        return false;
    };
    text.trim()
        .parse::<u64>()
        .map_or(true, |at| now_ms().saturating_sub(at) < ALIVE_GRACE_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_arrive_once_in_order() {
        let dir = std::env::temp_dir().join(format!("wf-control-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        send(&dir, &Command::Skip).unwrap();
        send(
            &dir,
            &Command::Approve {
                decision: Decision::Skip,
            },
        )
        .unwrap();
        std::fs::write(dir.join(COMMANDS_DIR).join("partial.tmp"), "{").unwrap();
        assert_eq!(
            take(&dir),
            [
                Command::Skip,
                Command::Approve {
                    decision: Decision::Skip
                }
            ]
        );
        assert!(take(&dir).is_empty());
        let inbox = dir.join(INBOX_DIR);
        steer(&inbox, "use the staging branch").unwrap();
        steer(&inbox, "then stop").unwrap();
        assert_eq!(
            take_steering(&inbox),
            ["use the staging branch", "then stop"]
        );
        touch(&dir, 7);
        assert!(alive(&dir, 7) && !alive(&dir, 8));
        std::fs::write(alive_file(&dir, 7), "1").unwrap();
        assert!(!alive(&dir, 7));
        forget(&dir, 7);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn decisions_parse() {
        assert_eq!(Decision::parse(""), Some(Decision::Run));
        assert_eq!(Decision::parse("Skip"), Some(Decision::Skip));
        assert_eq!(Decision::parse("maybe"), None);
    }
}
