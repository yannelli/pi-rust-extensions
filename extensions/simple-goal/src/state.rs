//! The simple goal, its session entry, and the shared extension state.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use pi_ext_sdk::prelude::*;
use serde::{Deserialize, Serialize};

use crate::config::{self, Config};

/// Session entry type and custom message type.
pub const ENTRY: &str = "simple-goal";
pub const STATUS_KEY: &str = "simple-goal";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Active,
    Paused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Goal {
    /// Creation time in ms; tells a late evaluation which goal it judged.
    pub id: u64,
    pub objective: String,
    #[serde(default)]
    pub done_when: Option<String>,
    pub status: Status,
    /// Continuations sent so far.
    #[serde(default)]
    pub rounds: u32,
    /// Continue verdicts in a row after runs without a tool call.
    #[serde(default)]
    pub idle_streak: u32,
    #[serde(default)]
    pub last_reason: Option<String>,
    #[serde(default)]
    pub paused_reason: Option<String>,
}

impl Goal {
    pub fn new(objective: String, done_when: Option<String>) -> Self {
        Self {
            id: now_ms(),
            objective,
            done_when: done_when.filter(|d| !d.trim().is_empty()),
            status: Status::Active,
            rounds: 0,
            idle_streak: 0,
            last_reason: None,
            paused_reason: None,
        }
    }

    pub fn pause(&mut self, reason: &str) {
        self.status = Status::Paused;
        self.paused_reason = Some(reason.to_owned());
    }

    pub fn resume(&mut self) {
        self.status = Status::Active;
        self.paused_reason = None;
        self.rounds = 0;
        self.idle_streak = 0;
    }
}

/// The persisted entry; `goal: null` records a cleared goal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub goal: Option<Goal>,
}

#[derive(Default)]
pub struct Inner {
    pub goal: Option<Goal>,
    /// Tool calls since the last `agent_settled`.
    pub tool_calls: u32,
    pub evaluating: bool,
    warned_untrusted: bool,
}

#[derive(Clone, Default)]
pub struct App {
    inner: Arc<Mutex<Inner>>,
}

impl App {
    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    pub fn goal(&self) -> Option<Goal> {
        self.lock().goal.clone()
    }

    /// Replaces the goal, appends the session entry, and redraws the status.
    pub async fn save(&self, host: &Host, goal: Option<Goal>) -> Result<()> {
        self.lock().goal = goal.clone();
        host.append_entry(ENTRY, &Entry { goal }).await?;
        self.refresh_status(host).await
    }

    /// Restores the latest entry of the current branch.
    pub async fn restore(&self, host: &Host) -> Result<()> {
        let entries: Vec<Entry> = host.entries_of(ENTRY).await.unwrap_or_default();
        {
            let mut inner = self.lock();
            inner.goal = entries.into_iter().last().and_then(|e| e.goal);
            inner.tool_calls = 0;
            inner.evaluating = false;
        }
        self.refresh_status(host).await
    }

    pub async fn refresh_status(&self, host: &Host) -> Result<()> {
        let max_rounds = self.config(host).await.max_rounds;
        let text = self
            .goal()
            .filter(|g| g.status == Status::Active)
            .map(|g| format!("simple goal {}/{max_rounds}", g.rounds));
        host.set_status(STATUS_KEY, text.as_deref()).await
    }

    /// Loads the config and warns once about project keys skipped in an untrusted project.
    pub async fn config(&self, host: &Host) -> Config {
        let trusted = project_trusted(host).await;
        let loaded = config::load(&host.agent_dir(), &host.project_dir(), trusted);
        if loaded.ignored.is_empty() {
            return loaded.config;
        }
        let first = !std::mem::replace(&mut self.lock().warned_untrusted, true);
        if first {
            let keys = loaded.ignored.join(", ");
            let message = format!(
                "simple-goal: the project is not trusted; ignoring project simple-goal.json keys {keys}"
            );
            pi_ext_sdk::warn!("{message}");
            let _ = host.notify_warning(message).await;
        }
        loaded.config
    }
}

async fn project_trusted(host: &Host) -> bool {
    let from_snapshot = host
        .snapshot()
        .and_then(|s| s.extra.get("isProjectTrusted").and_then(Value::as_bool));
    if let Some(trusted) = from_snapshot {
        return trusted;
    }
    let answer = host.pi_call("ctx", "isProjectTrusted", vec![]).await;
    answer.ok().and_then(|v| v.as_bool()).unwrap_or(false)
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Text of the `status` tool action and of `/simple-goal` without arguments.
pub fn describe(goal: Option<&Goal>, max_rounds: u32) -> String {
    let Some(goal) = goal else {
        return "No simple goal.".into();
    };
    let mut lines = vec![match goal.status {
        Status::Active => format!("Simple goal active, round {}/{max_rounds}.", goal.rounds),
        Status::Paused => format!(
            "Simple goal paused ({}). /simple-goal resume continues.",
            goal.paused_reason.as_deref().unwrap_or("paused")
        ),
    }];
    lines.push(format!("Objective: {}", goal.objective));
    if let Some(done_when) = &goal.done_when {
        lines.push(format!("Done when: {done_when}"));
    }
    if let Some(reason) = &goal.last_reason {
        lines.push(format!("Last evaluation: {reason}"));
    }
    lines.join("\n")
}
