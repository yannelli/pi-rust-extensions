//! Todo data: tasks, phases, the state snapshot, and the `details` envelope.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Deleted,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Deleted => "deleted",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "pending" => Self::Pending,
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            "deleted" => Self::Deleted,
            _ => return None,
        })
    }

    /// rpiv-todo transition table; a same-status transition is accepted.
    pub fn can_become(self, to: Self) -> bool {
        use TaskStatus::*;
        self == to
            || matches!(
                (self, to),
                (Pending, InProgress | Completed | Deleted)
                    | (InProgress, Pending | Completed | Deleted)
                    | (Completed, Deleted)
            )
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Self::Pending => "○",
            Self::InProgress => "◐",
            Self::Completed => "✓",
            Self::Deleted => "✗",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: u64,
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<u64>,
}

impl Task {
    pub fn new(id: u64, subject: impl Into<String>) -> Self {
        Self {
            id,
            subject: subject.into(),
            description: None,
            active_form: None,
            status: TaskStatus::Pending,
            blocked_by: Vec::new(),
            owner: None,
            metadata: None,
            phase: None,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.status != TaskStatus::Deleted
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseStatus {
    Pending,
    InProgress,
    Completed,
}

impl PhaseStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "pending" => Self::Pending,
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            _ => return None,
        })
    }

    pub fn next(self) -> Self {
        match self {
            Self::Pending => Self::InProgress,
            Self::InProgress => Self::Completed,
            Self::Completed => Self::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Phase {
    pub id: u64,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    pub status: PhaseStatus,
}

fn is_one(n: &u64) -> bool {
    *n == 1
}

fn one() -> u64 {
    1
}

/// The todo list of one scope. Without phases it matches rpiv-todo's `TaskState`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoState {
    pub tasks: Vec<Task>,
    pub next_id: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phases: Vec<Phase>,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub next_phase_id: u64,
}

impl Default for TodoState {
    fn default() -> Self {
        Self {
            tasks: Vec::new(),
            next_id: 1,
            phases: Vec::new(),
            next_phase_id: 1,
        }
    }
}

impl TodoState {
    pub fn task(&self, id: u64) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    pub fn phase(&self, id: u64) -> Option<&Phase> {
        self.phases.iter().find(|p| p.id == id)
    }

    pub fn visible_tasks(&self) -> impl Iterator<Item = &Task> {
        self.tasks.iter().filter(|t| t.is_visible())
    }

    /// The first phase in progress, else the first pending phase, else the last phase.
    pub fn current_phase(&self) -> Option<&Phase> {
        let find = |status| self.phases.iter().find(|p| p.status == status);
        find(PhaseStatus::InProgress)
            .or_else(|| find(PhaseStatus::Pending))
            .or_else(|| self.phases.last())
    }

    /// `(completed, total)` over visible tasks.
    pub fn counts<'a>(tasks: impl Iterator<Item = &'a Task>) -> (usize, usize) {
        tasks
            .filter(|t| t.is_visible())
            .fold((0, 0), |(done, total), t| {
                let done = done + usize::from(t.status == TaskStatus::Completed);
                (done, total + 1)
            })
    }
}

/// The tool result `details`, also the `rpiv-todo-state` entry data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDetails {
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub params: Value,
    #[serde(flatten)]
    pub state: TodoState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn transitions_follow_rpiv_table() {
        use TaskStatus::*;
        assert!(Pending.can_become(InProgress));
        assert!(InProgress.can_become(Pending));
        assert!(Completed.can_become(Deleted));
        assert!(Completed.can_become(Completed));
        assert!(!Completed.can_become(InProgress));
        assert!(!Deleted.can_become(Pending));
    }

    #[test]
    fn details_round_trip_without_phases_matches_upstream_shape() {
        let mut state = TodoState::default();
        state.tasks.push(Task::new(1, "A"));
        state.next_id = 2;
        let details = TaskDetails {
            action: "create".into(),
            params: json!({ "subject": "A" }),
            state,
            error: None,
            scope: None,
        };
        let value = serde_json::to_value(&details).unwrap();
        assert_eq!(
            value,
            json!({ "action": "create", "params": { "subject": "A" },
                "tasks": [{ "id": 1, "subject": "A", "status": "pending" }], "nextId": 2 })
        );
        let back: TaskDetails = serde_json::from_value(value).unwrap();
        assert_eq!(back, details);
    }
}
