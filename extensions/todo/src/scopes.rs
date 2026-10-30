//! Subagent scopes: todo lists of child runs reported on the `todo:scope` channel.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{TaskDetails, TodoState};

pub const SCOPE_CHANNEL: &str = "todo:scope";
pub const SCOPES_ENTRY_TYPE: &str = "pi-todo-scopes";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Visibility {
    All,
    MainOnly,
    #[default]
    Collapsed,
}

impl Visibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::MainOnly => "main-only",
            Self::Collapsed => "collapsed",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::All => Self::Collapsed,
            Self::Collapsed => Self::MainOnly,
            Self::MainOnly => Self::All,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl ScopeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Self::Running => "◆",
            Self::Completed => "✓",
            Self::Failed => "✗",
            Self::Cancelled => "⊘",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scope {
    pub scope: String,
    pub label: String,
    pub status: ScopeStatus,
    #[serde(default)]
    pub state: TodoState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Visibility>,
}

/// Payload of a `todo:scope` message.
#[derive(Debug, Clone, Deserialize)]
pub struct ScopeEvent {
    pub scope: String,
    #[serde(default)]
    pub label: Option<String>,
    pub status: ScopeStatus,
    #[serde(default)]
    pub state: Option<Value>,
}

/// All scopes in arrival order; the `pi-todo-scopes` entry data.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Scopes {
    pub scopes: Vec<Scope>,
}

impl Scopes {
    pub fn get(&self, id: &str) -> Option<&Scope> {
        self.scopes.iter().find(|s| s.scope == id)
    }

    /// Applies one channel message; returns false when it changed nothing.
    pub fn apply(&mut self, event: ScopeEvent) -> bool {
        let state = event
            .state
            .and_then(|v| serde_json::from_value::<TaskDetails>(v).ok())
            .map(|d| d.state);
        let before = self.clone();
        match self.scopes.iter_mut().find(|s| s.scope == event.scope) {
            Some(scope) => {
                scope.status = event.status;
                if let Some(label) = event.label.filter(|l| !l.is_empty()) {
                    scope.label = label;
                }
                if let Some(state) = state {
                    scope.state = state;
                }
            }
            None => self.scopes.push(Scope {
                label: event.label.unwrap_or_else(|| event.scope.clone()),
                scope: event.scope,
                status: event.status,
                state: state.unwrap_or_default(),
                visibility: None,
            }),
        }
        *self != before
    }

    pub fn visibility(&self, id: &str, default: Visibility) -> Visibility {
        self.get(id).and_then(|s| s.visibility).unwrap_or(default)
    }

    pub fn set_visibility(&mut self, id: &str, visibility: Visibility) -> bool {
        match self.scopes.iter_mut().find(|s| s.scope == id) {
            Some(scope) if scope.visibility != Some(visibility) => {
                scope.visibility = Some(visibility);
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(value: Value) -> ScopeEvent {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn applies_state_and_status_updates() {
        let mut scopes = Scopes::default();
        let details = json!({ "action": "create", "params": {}, "nextId": 2,
            "tasks": [{ "id": 1, "subject": "Scan", "status": "pending" }], "scope": "r1" });
        assert!(
            scopes.apply(event(json!({ "scope": "r1", "label": "explore: scan",
            "status": "running", "state": details })))
        );
        assert_eq!(scopes.get("r1").unwrap().state.tasks.len(), 1);
        assert!(scopes.apply(event(
            json!({ "scope": "r1", "status": "completed", "state": null })
        )));
        let scope = scopes.get("r1").unwrap();
        assert_eq!(scope.status, ScopeStatus::Completed);
        assert_eq!(scope.label, "explore: scan");
        assert_eq!(scope.state.tasks.len(), 1);
        assert!(!scopes.apply(event(json!({ "scope": "r1", "status": "completed" }))));
    }

    #[test]
    fn visibility_defaults_and_overrides() {
        let mut scopes = Scopes::default();
        scopes.apply(event(json!({ "scope": "r1", "status": "running" })));
        assert_eq!(scopes.visibility("r1", Visibility::All), Visibility::All);
        assert!(scopes.set_visibility("r1", Visibility::MainOnly));
        assert!(!scopes.set_visibility("r1", Visibility::MainOnly));
        assert_eq!(
            scopes.visibility("r1", Visibility::All),
            Visibility::MainOnly
        );
        assert_eq!(
            serde_json::to_value(Visibility::MainOnly).unwrap(),
            json!("main-only")
        );
    }
}
