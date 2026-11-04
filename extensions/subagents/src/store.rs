//! Run directories under `<agentDir>/ext/subagents/runs`: child session files, the async
//! `status.json`, and removal of old run directories.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use pi_ext_sdk::prelude::*;

use crate::runs::{Run, Status};

pub const RETENTION: Duration = Duration::from_secs(14 * 24 * 60 * 60);
pub const STATUS_FILE: &str = "status.json";

pub fn runs_root(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext/subagents/runs")
}

pub fn session_file(root: &Path, run_id: &str) -> PathBuf {
    root.join(run_id).join("session.jsonl")
}

/// Deletes run directories in `root` last modified more than `max_age` ago.
pub fn prune(root: &Path, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let expired = entry
            .metadata()
            .ok()
            .filter(|meta| meta.is_dir())
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > max_age);
        if expired && let Err(err) = std::fs::remove_dir_all(entry.path()) {
            pi_ext_sdk::warn!("cannot remove {}: {err}", entry.path().display());
        }
    }
}

/// `running` until every run ends, then `complete`, `stopped` (any cancelled), or `failed`.
pub fn call_state(runs: &[Run]) -> &'static str {
    if runs.iter().any(|r| r.status == Status::Running) {
        "running"
    } else if runs.iter().all(|r| r.status == Status::Completed) {
        "complete"
    } else if runs.iter().any(|r| r.status == Status::Cancelled) {
        "stopped"
    } else {
        "failed"
    }
}

pub fn step_status(status: Status) -> &'static str {
    match status {
        Status::Running => "running",
        Status::Completed => "completed",
        Status::Failed => "failed",
        Status::Cancelled => "stopped",
    }
}

pub fn status_json(runs: &[Run]) -> Value {
    let steps: Vec<Value> = runs
        .iter()
        .map(|r| {
            json!({ "agent": r.agent, "status": step_status(r.status), "sessionFile": r.session_file })
        })
        .collect();
    json!({ "state": call_state(runs), "steps": steps })
}

/// Writes `<dir>/status.json` through a rename, so readers see a whole file.
pub fn write_status(dir: &Path, runs: &[Run]) {
    let path = dir.join(STATUS_FILE);
    let temp = dir.join(format!("{STATUS_FILE}.tmp"));
    let written = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&temp, status_json(runs).to_string()))
        .and_then(|()| std::fs::rename(&temp, &path));
    if let Err(err) = written {
        pi_ext_sdk::warn!("cannot write {}: {err}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::Runs;
    use pi_ext_sdk::CancellationToken;

    fn runs_with(statuses: &[Status]) -> Vec<Run> {
        let runs = Runs::default();
        for status in statuses {
            let id = runs.start("explore", "t", true, CancellationToken::new());
            if *status != Status::Running {
                runs.finish(&id, *status, 10);
            }
        }
        runs.all()
    }

    #[test]
    fn call_state_follows_the_runs() {
        use Status::*;
        let cases: [(&[Status], &str); 5] = [
            (&[Running, Completed], "running"),
            (&[Completed, Completed], "complete"),
            (&[Completed, Failed], "failed"),
            (&[Failed, Cancelled], "stopped"),
            (&[Cancelled], "stopped"),
        ];
        for (statuses, state) in cases {
            assert_eq!(call_state(&runs_with(statuses)), state, "{statuses:?}");
        }
        let steps = status_json(&runs_with(&[Cancelled, Failed]))["steps"].clone();
        assert_eq!(steps[0]["status"], "stopped");
        assert_eq!(steps[1]["status"], "failed");
    }

    #[cfg(unix)]
    #[test]
    fn prune_removes_only_old_run_directories() {
        let root = std::env::temp_dir().join(format!("subagents-prune-{}", std::process::id()));
        let (old, new) = (root.join("old"), root.join("new"));
        for dir in [&old, &new] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let past = SystemTime::now() - RETENTION - Duration::from_secs(60);
        std::fs::File::open(&old)
            .unwrap()
            .set_modified(past)
            .unwrap();
        prune(&root, RETENTION);
        assert!(!old.exists());
        assert!(new.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
