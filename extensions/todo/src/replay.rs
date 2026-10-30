//! Rebuilds state from the session branch (rpiv-todo replay rules plus scopes).

use serde_json::Value;

use crate::model::{TaskDetails, TodoState};
use crate::scopes::{SCOPES_ENTRY_TYPE, Scopes};

pub const STATE_ENTRY_TYPE: &str = "rpiv-todo-state";
pub const TOOL_NAME: &str = "todo";

fn details_of(value: &Value) -> Option<TodoState> {
    serde_json::from_value::<TaskDetails>(value.clone())
        .ok()
        .map(|d| d.state)
}

/// The last `rpiv-todo-state` entry wins; without one, the last `todo` tool result details.
pub fn replay(entries: &[Value]) -> (TodoState, Scopes) {
    let mut from_entries = None;
    let mut from_results = None;
    let mut scopes = None;
    for entry in entries {
        match entry.get("type").and_then(Value::as_str) {
            Some("custom") => {
                let data = entry.get("data").unwrap_or(&Value::Null);
                match entry.get("customType").and_then(Value::as_str) {
                    Some(STATE_ENTRY_TYPE) => from_entries = details_of(data).or(from_entries),
                    Some(SCOPES_ENTRY_TYPE) => {
                        scopes = serde_json::from_value::<Scopes>(data.clone())
                            .ok()
                            .or(scopes)
                    }
                    _ => {}
                }
            }
            Some("message") => {
                let message = entry.get("message").unwrap_or(&Value::Null);
                let is_todo = message.get("role").and_then(Value::as_str) == Some("toolResult")
                    && message.get("toolName").and_then(Value::as_str) == Some(TOOL_NAME);
                if is_todo && let Some(details) = message.get("details") {
                    from_results = details_of(details).or(from_results);
                }
            }
            _ => {}
        }
    }
    let state = from_entries.or(from_results).unwrap_or_default();
    (state, scopes.unwrap_or_default())
}
