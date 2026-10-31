//! Shared runtime state and helpers used by hooks, commands, and tools.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use pi_ext_sdk::{DeliverAs, Host, Snapshot};
use serde_json::{Value, json};

use crate::config::{self, Config, Method};
use crate::plan::Stats;
use crate::trigger::AutoState;

pub const COMPACT_THEN: &str = "unipi:compact-then";
pub const COMPACTED_CHANNEL: &str = "unipi:compactor:compacted";
pub const ACTIVE_WORK_CHANNEL: &str = "compactor:active-work";
pub const ACTIVE_WORK_SYNC: &str = "compactor:active-work:sync";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueMode {
    Steer,
    FollowUp,
}

#[derive(Debug, Default)]
pub struct State {
    pub pending_method: Option<Method>,
    pub pending_command: Option<String>,
    pub command_compaction: bool,
    pub last_command: Option<String>,
    pub last_reason: Option<String>,
    pub last_stats: Option<Stats>,
    pub last_details: Option<Value>,
    pub pending_follow_up: Option<String>,
    pub compact_then: Option<(String, QueueMode)>,
    pub auto: AutoState,
    pub active_work: BTreeMap<String, String>,
    /// The session the `active_work` blocks belong to; blocks of another session are dropped.
    pub active_session: Option<String>,
    pub recall_queries: u64,
    pub compactions: u64,
}

impl State {
    /// Routes the next compaction to `method` (`None` clears it).
    pub fn set_pending(
        &mut self,
        method: Option<Method>,
        from_command: bool,
        command: Option<&str>,
    ) {
        self.pending_method = method;
        self.command_compaction = method.is_some() && from_command;
        self.pending_command = method.and(command.map(str::to_owned));
    }

    /// A new or switched session: drop the blocks of the previous one.
    pub fn start_active_work(&mut self, session: Option<String>) {
        self.active_work.clear();
        self.active_session = session;
    }

    /// Keeps or removes block `id`; `false` when `session` is another session's.
    pub fn set_active_work(&mut self, id: &str, text: &str, session: Option<&str>) -> bool {
        let current = self.active_session.as_deref();
        if session
            .zip(current)
            .is_some_and(|(sent, current)| sent != current)
        {
            return false;
        }
        if text.is_empty() {
            self.active_work.remove(id);
        } else {
            self.active_work.insert(id.to_owned(), text.to_owned());
        }
        true
    }

    pub fn active_work(&self) -> Vec<(String, String)> {
        self.active_work
            .iter()
            .map(|(id, text)| (id.clone(), text.clone()))
            .collect()
    }
}

#[derive(Clone, Default)]
pub struct Shared(Arc<Mutex<State>>);

impl Shared {
    pub fn lock(&self) -> MutexGuard<'_, State> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub fn load_config(host: &Host, snapshot: &Snapshot) -> Config {
    let cwd = if snapshot.cwd.is_empty() {
        host.cwd()
    } else {
        snapshot.cwd_path()
    };
    config::load(&host.agent_dir(), &cwd)
}

pub fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

/// Writes the last plan to `<agentDir>/ext/compactor/debug.json` (mode 0600) when `debug` is on.
pub fn debug_dump(host: &Host, config: &Config, data: Value) {
    if !config.debug {
        return;
    }
    let path = debug_path(&host.agent_dir());
    let text = serde_json::to_string_pretty(&data).unwrap_or_default();
    if let Err(err) = write_private(&path, &text) {
        pi_ext_sdk::warn!("cannot write {}: {err}", path.display());
    }
}

pub fn debug_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join("compactor").join("debug.json")
}

/// Replaces `path` with a new file; a symlink at `path` is removed, not followed.
pub fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match std::fs::remove_file(path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
        _ => {}
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(text.as_bytes())
}

/// Counts a compaction and announces it on the `pi.events` bus.
pub async fn after_compaction(host: &Host, shared: &Shared, stats: Option<&Stats>, method: Method) {
    shared.lock().compactions += 1;
    let before = stats.and_then(|s| s.tokens_before).unwrap_or(0);
    let after = stats.map_or(0, |s| s.tokens_after_est);
    let payload = json!({
        "method": method.as_str(),
        "summarized": stats.map_or(0, |s| s.summarized),
        "kept": stats.map_or(0, |s| s.kept),
        "tokensBefore": before,
        "tokensAfter": after,
        "tokensSaved": before.saturating_sub(after),
    });
    if let Err(err) = host.emit(COMPACTED_CHANNEL, payload).await {
        pi_ext_sdk::debug!("events.emit failed: {}", err.message());
    }
}

/// Sends a prompt as a user message; failures go to the log.
pub async fn send_prompt(host: &Host, prompt: &str, deliver: DeliverAs) {
    if let Err(err) = host.send_user_message(prompt, Some(deliver)).await {
        pi_ext_sdk::warn!("could not send the follow-up prompt: {}", err.message());
    }
}

/// Runs `ctx.compact` in its own task: it waits for the compaction, which runs the hooks of this extension.
pub fn spawn_compact(
    host: Host,
    shared: Shared,
    instructions: Option<String>,
    follow_up: Option<String>,
    quiet_nothing: bool,
) {
    tokio::spawn(async move {
        match host.compact(instructions.as_deref()).await {
            Ok(_) => {
                shared.lock().set_pending(None, true, None);
                if let Some(prompt) = follow_up.filter(|p| !p.is_empty()) {
                    send_prompt(&host, &prompt, DeliverAs::FollowUp).await;
                }
            }
            Err(err) => {
                shared.lock().set_pending(None, true, None);
                let message = err.message().to_owned();
                let note = if message.to_lowercase().contains("too small") && quiet_nothing {
                    None
                } else if quiet_nothing
                    && (message.contains("Compaction cancelled")
                        || message.contains("Already compacted"))
                {
                    Some(host.notify_info("Nothing to compact.").await)
                } else {
                    Some(
                        host.notify_error(format!("Compaction failed: {message}"))
                            .await,
                    )
                };
                if let Some(Err(err)) = note {
                    pi_ext_sdk::debug!("notify failed: {}", err.message());
                }
            }
        }
    });
}

/// Asks producers to send their current `[Active Work]` blocks again, for the current session.
pub async fn request_active_work(host: &Host, shared: &Shared) {
    let session = shared.lock().active_session.clone();
    if let Err(err) = host
        .emit(ACTIVE_WORK_SYNC, json!({ "sessionId": session }))
        .await
    {
        pi_ext_sdk::debug!("cannot request active work: {err}");
    }
}

/// Keeps `{ id, text, sessionId? }` blocks on `compactor:active-work`.
/// Requests current blocks after subscribing to recover earlier messages.
pub async fn watch_active_work(host: Host, shared: Shared) -> pi_ext_sdk::Result<()> {
    let mut subscription = host.subscribe(ACTIVE_WORK_CHANNEL).await?;
    request_active_work(&host, &shared).await;
    while let Some(data) = subscription.recv().await {
        let Some(id) = data["id"].as_str() else {
            continue;
        };
        let text = data["text"].as_str().map(str::trim).unwrap_or_default();
        shared
            .lock()
            .set_active_work(id, text, data["sessionId"].as_str());
    }
    Ok(())
}

#[cfg(test)]
mod active_work_tests {
    use super::State;

    #[test]
    fn blocks_of_another_session_are_dropped_and_a_switch_clears_the_rest() {
        let mut state = State::default();
        state.start_active_work(Some("s1".into()));
        assert!(state.set_active_work("goal", "Goal: a", Some("s1")));
        assert!(state.set_active_work("tasks", "Tasks: b", None));
        state.start_active_work(Some("s2".into()));
        assert!(state.active_work().is_empty());
        assert!(!state.set_active_work("goal", "Goal: a", Some("s1")));
        assert!(state.set_active_work("goal", "Goal: c", Some("s2")));
        assert!(state.set_active_work("goal", "", Some("s2")));
        assert!(state.active_work().is_empty());
    }
}
