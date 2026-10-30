use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use pi_ext_sdk::{Host, Result, Snapshot, info, warn};
use serde_json::json;
use tokio::sync::{Mutex, MutexGuard};

use crate::checkpoint::{self, Checkpoint, ENTRY_TYPE};
use crate::config::Config;
use crate::git::ShadowRepo;

#[derive(Debug, Clone)]
pub struct Pending {
    pub user_entry_id: String,
    pub prompt: String,
    pub before_commit: String,
}

pub struct Session {
    pub config: Config,
    pub repo: ShadowRepo,
    pub storage: PathBuf,
    pub pending: Option<Pending>,
    pub last_commit: Option<String>,
    pub tree_target: Option<Option<String>>,
}

impl Session {
    pub fn new(config: Config, storage: PathBuf, work_tree: &Path) -> Self {
        Self {
            repo: ShadowRepo::new(&storage, work_tree),
            config,
            storage,
            pending: None,
            last_commit: None,
            tree_target: None,
        }
    }

    pub async fn snapshot(&mut self, message: &str, base: Option<&str>) -> Result<String> {
        self.repo.ensure().await?;
        let commit = self
            .repo
            .snapshot(
                message,
                base,
                &self.config.exclude,
                self.config.max_file_bytes,
            )
            .await?;
        self.last_commit = Some(commit.clone());
        Ok(commit)
    }

    /// Captures the pre-turn state for `user_entry_id` unless that turn already started.
    pub async fn begin_turn(
        &mut self,
        host: &Host,
        user_entry_id: &str,
        prompt: &str,
    ) -> Result<()> {
        if self.pending.as_ref().map(|p| p.user_entry_id.as_str()) == Some(user_entry_id) {
            return Ok(());
        }
        if let Err(err) = self.finish_turn(host).await {
            warn!("dropped unfinished checkpoint: {err}");
            self.pending = None;
        }
        let base = self.last_commit.clone();
        let before = self
            .snapshot(&format!("[pi] before:{user_entry_id}"), base.as_deref())
            .await?;
        self.pending = Some(Pending {
            user_entry_id: user_entry_id.to_string(),
            prompt: prompt.to_string(),
            before_commit: before,
        });
        Ok(())
    }

    /// Captures the post-turn state and appends the `pi-checkpoint` entry.
    /// The turn stays pending when a step fails, so the next call retries it.
    pub async fn finish_turn(&mut self, host: &Host) -> Result<Option<Checkpoint>> {
        let Some(pending) = self.pending.clone() else {
            return Ok(None);
        };
        let message = format!("[pi] entry:{}", pending.user_entry_id);
        let after = self
            .snapshot(&message, Some(&pending.before_commit))
            .await?;
        let file_changes = self.repo.numstat(&pending.before_commit, &after).await?;
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let checkpoint = Checkpoint {
            v: 2,
            kind: "checkpoint".into(),
            turn_id: format!("{}-{created_at}", pending.user_entry_id),
            user_entry_id: pending.user_entry_id,
            before_commit: pending.before_commit,
            after_commit: after,
            prompt: pending.prompt,
            file_count: file_changes.len(),
            file_changes,
            created_at: json!(created_at),
        };
        host.append_entry(ENTRY_TYPE, &checkpoint).await?;
        self.pending = None;
        info!(
            "checkpoint {} {}..{} files={}",
            checkpoint.user_entry_id,
            short(&checkpoint.before_commit),
            short(&checkpoint.after_commit),
            checkpoint.file_count
        );
        Ok(Some(checkpoint))
    }

    /// Writes `commit` into the work tree; rolls back to a safety snapshot on failure.
    pub async fn restore(&mut self, commit: &str) -> Result<()> {
        let safety = self.snapshot("[pi] safety", None).await?;
        if let Err(err) = self.repo.checkout(commit).await {
            let _ = self.repo.checkout(&safety).await;
            return Err(err);
        }
        self.last_commit = Some(commit.to_string());
        Ok(())
    }

    /// True when the work tree equals `commit`.
    pub async fn matches(&mut self, commit: &str) -> Result<bool> {
        self.repo.ensure().await?;
        self.repo
            .stage_all(&self.config.exclude, self.config.max_file_bytes)
            .await?;
        Ok(self
            .repo
            .git(&["diff", "--cached", "--quiet", commit])
            .await
            .is_ok())
    }
}

fn short(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

/// `<agentDir>/ext/rewind/sessions/<session file stem>` or `ephemeral-<session id>`.
pub fn storage_dir(agent_dir: &Path, snapshot: &Snapshot) -> PathBuf {
    sessions_dir(agent_dir).join(session_key(
        snapshot.session_file.as_deref(),
        snapshot.session_id.as_deref(),
    ))
}

pub fn sessions_dir(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join("rewind").join("sessions")
}

pub fn session_key(session_file: Option<&str>, session_id: Option<&str>) -> String {
    let stem = session_file
        .filter(|file| !file.is_empty())
        .and_then(|file| Path::new(file).file_stem())
        .map(|stem| stem.to_string_lossy().into_owned());
    let key = stem.unwrap_or_else(|| format!("ephemeral-{}", session_id.unwrap_or("default")));
    key.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[derive(Default)]
pub struct State {
    session: Mutex<Option<Session>>,
    suppress_tree: AtomicBool,
    warning: AtomicBool,
}

impl State {
    pub async fn lock(&self) -> MutexGuard<'_, Option<Session>> {
        self.session.lock().await
    }

    pub fn set_suppress_tree(&self, on: bool) {
        self.suppress_tree.store(on, Ordering::SeqCst);
    }

    pub fn tree_suppressed(&self) -> bool {
        self.suppress_tree.load(Ordering::SeqCst)
    }

    pub fn set_warning(&self, on: bool) -> bool {
        self.warning.swap(on, Ordering::SeqCst)
    }
}

/// Latest `afterCommit` of the checkpoints on `branch`.
pub fn latest_after(branch: &[serde_json::Value]) -> Option<String> {
    checkpoint::checkpoints(branch)
        .last()
        .map(|cp| cp.after_commit.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_keys_are_safe() {
        assert_eq!(session_key(Some("/s/2026_abc.jsonl"), None), "2026_abc");
        assert_eq!(session_key(None, Some("x/y")), "ephemeral-x_y");
        assert_eq!(session_key(Some(""), None), "ephemeral-default");
    }
}
