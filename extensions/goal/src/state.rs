use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::format::extract_verification_contract;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    BudgetLimited,
    Auditing,
    Complete,
}

impl GoalStatus {
    pub fn is_open(self) -> bool {
        self != Self::Complete
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    User,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[default]
    Pending,
    InProgress,
    Complete,
    Skipped,
}

impl TaskStatus {
    pub fn is_resolved(self) -> bool {
        matches!(self, Self::Complete | Self::Skipped)
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::InProgress => "[>]",
            Self::Complete => "[x]",
            Self::Skipped => "[~]",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GoalTask {
    pub id: String,
    pub title: String,
    pub parent_id: Option<String>,
    pub verification_contract: Option<String>,
    pub lightweight_subtasks: bool,
    pub status: TaskStatus,
    pub evidence: Option<String>,
    pub skip_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Usage {
    pub tokens_used: u64,
    pub active_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub at: u64,
    pub event: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Goal {
    pub id: String,
    pub objective: String,
    #[serde(default)]
    pub sisyphus: bool,
    pub status: GoalStatus,
    #[serde(default)]
    pub stop_reason: Option<StopReason>,
    #[serde(default = "yes")]
    pub auto_continue: bool,
    #[serde(default)]
    pub token_budget: Option<u64>,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub tasks: Vec<GoalTask>,
    #[serde(default)]
    pub block_completion: bool,
    #[serde(default)]
    pub current_task_id: Option<String>,
    #[serde(default)]
    pub verification_contract: Option<String>,
    #[serde(default)]
    pub runs: u32,
    #[serde(default)]
    pub audits: u32,
    #[serde(default)]
    pub last_audit: Option<String>,
    #[serde(default)]
    pub completion_summary: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub wait: Option<WaitState>,
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitState {
    pub reason: String,
    pub deadline: String,
    pub deadline_ms: u64,
    pub wait_id: Option<String>,
}

fn yes() -> bool {
    true
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("g{:x}", (nanos ^ (n << 48)) & 0xffff_ffff_ffff)
}

impl Goal {
    pub fn new(objective: &str, sisyphus: bool, token_budget: Option<u64>) -> Self {
        let now = now_ms();
        let mut goal = Self {
            id: new_id(),
            objective: objective.trim().to_owned(),
            sisyphus,
            status: GoalStatus::Active,
            stop_reason: None,
            auto_continue: true,
            token_budget,
            usage: Usage::default(),
            tasks: Vec::new(),
            block_completion: false,
            current_task_id: None,
            verification_contract: extract_verification_contract(objective),
            runs: 0,
            audits: 0,
            last_audit: None,
            completion_summary: None,
            created_at: now,
            updated_at: now,
            wait: None,
            history: Vec::new(),
        };
        goal.log("created");
        goal
    }

    pub fn log(&mut self, event: impl Into<String>) {
        self.updated_at = now_ms();
        self.history.push(HistoryEntry {
            at: self.updated_at,
            event: event.into(),
        });
        let excess = self.history.len().saturating_sub(200);
        self.history.drain(..excess);
    }

    pub fn set_status(&mut self, status: GoalStatus, reason: Option<StopReason>) {
        self.status = status;
        self.stop_reason = reason;
        self.updated_at = now_ms();
    }

    pub fn budget_reached(&self) -> bool {
        self.token_budget
            .is_some_and(|budget| self.usage.tokens_used >= budget)
    }

    pub fn task(&self, id: &str) -> Option<&GoalTask> {
        self.tasks.iter().find(|task| task.id == id)
    }

    pub fn children<'a>(&'a self, parent: Option<&'a str>) -> impl Iterator<Item = &'a GoalTask> {
        self.tasks
            .iter()
            .filter(move |task| task.parent_id.as_deref() == parent)
    }

    /// Tasks in tree order with their depth.
    pub fn ordered_tasks(&self) -> Vec<(usize, &GoalTask)> {
        let mut out = Vec::new();
        self.walk(None, 0, &mut out);
        out
    }

    fn walk<'a>(
        &'a self,
        parent: Option<&'a str>,
        depth: usize,
        out: &mut Vec<(usize, &'a GoalTask)>,
    ) {
        if depth > 16 {
            return;
        }
        for task in self.children(parent) {
            out.push((depth, task));
            self.walk(Some(&task.id), depth + 1, out);
        }
    }

    pub fn counts(&self) -> TaskCounts {
        let mut counts = TaskCounts::default();
        for task in &self.tasks {
            counts.total += 1;
            match task.status {
                TaskStatus::Complete => counts.complete += 1,
                TaskStatus::Skipped => counts.skipped += 1,
                _ => counts.pending += 1,
            }
        }
        counts
    }

    /// Pending tasks that block completion: every task when `block_completion`, children
    /// of lightweight parents excepted.
    pub fn gating_pending(&self) -> Vec<&GoalTask> {
        if !self.block_completion {
            return Vec::new();
        }
        self.tasks
            .iter()
            .filter(|task| !task.status.is_resolved())
            .filter(|task| {
                let parent = task.parent_id.as_deref().and_then(|id| self.task(id));
                !parent.is_some_and(|parent| parent.lightweight_subtasks)
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TaskCounts {
    pub total: usize,
    pub complete: usize,
    pub skipped: usize,
    pub pending: usize,
}
