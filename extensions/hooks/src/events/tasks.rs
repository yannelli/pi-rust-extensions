use std::sync::Arc;

use pi_ext_sdk::prelude::*;

use crate::decide::{contexts, first_block};
use crate::dispatch::{Dispatch, dispatch};
use crate::matcher::Target;
use crate::runtime::Runtime;

/// The pi `todo` tool (extension `todo`) plays the role of the Claude Task tools.
pub const TODO_TOOL: &str = "todo";

/// One task the `todo` call creates or completes.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskInfo {
    pub id: String,
    pub subject: String,
    pub description: Option<String>,
    pub owner: Option<String>,
}

/// The latest `todo` result `details` on the branch: `{ tasks: [...], nextId }`.
async fn todo_state(host: &Host) -> Value {
    let branch = host.branch(None).await.unwrap_or_default();
    branch
        .iter()
        .rev()
        .filter_map(|entry| entry.get("message"))
        .find(|m| {
            m.get("role").and_then(Value::as_str) == Some("toolResult")
                && m.get("toolName").and_then(Value::as_str) == Some(TODO_TOOL)
                && m.pointer("/details/tasks").is_some()
        })
        .and_then(|m| m.get("details").cloned())
        .unwrap_or(Value::Null)
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// Tasks a `create` call adds; ids continue from the last `nextId` (or the highest id + 1).
pub fn created_tasks(input: &Value, state: &Value) -> Vec<TaskInfo> {
    let tasks = state.get("tasks").and_then(Value::as_array);
    let highest = tasks
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("id").and_then(Value::as_u64))
        .max()
        .unwrap_or(0);
    let mut next = state
        .get("nextId")
        .and_then(Value::as_u64)
        .unwrap_or(highest + 1);
    let items: Vec<Value> = match input.get("tasks").and_then(Value::as_array) {
        Some(items) => items.clone(),
        None => vec![input.clone()],
    };
    items
        .iter()
        .filter_map(|item| {
            let subject = text(item, "subject")?;
            let info = TaskInfo {
                id: next.to_string(),
                subject,
                description: text(item, "description"),
                owner: text(item, "owner"),
            };
            next += 1;
            Some(info)
        })
        .collect()
}

/// The task an `update` call marks completed, with its subject from the last state.
pub fn completed_task(input: &Value, state: &Value) -> Option<TaskInfo> {
    if text(input, "status").as_deref() != Some("completed") {
        return None;
    }
    let id = input.get("id").and_then(Value::as_u64)?;
    let known = state
        .get("tasks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|t| t.get("id").and_then(Value::as_u64) == Some(id));
    Some(TaskInfo {
        id: id.to_string(),
        subject: text(input, "subject")
            .or_else(|| known.and_then(|t| text(t, "subject")))
            .unwrap_or_default(),
        description: text(input, "description")
            .or_else(|| known.and_then(|t| text(t, "description"))),
        owner: text(input, "owner").or_else(|| known.and_then(|t| text(t, "owner"))),
    })
}

fn payload(task: &TaskInfo) -> Value {
    let mut payload = json!({ "task_id": task.id, "task_subject": task.subject });
    if let Some(description) = &task.description {
        payload["task_description"] = json!(description);
    }
    if let Some(owner) = &task.owner {
        payload["teammate_name"] = json!(owner);
    }
    payload
}

/// TaskCreated or TaskCompleted for a `todo` call: the block reason, or context for the result.
pub async fn todo_call(
    rt: &Arc<Runtime>,
    host: &Host,
    snap: &Snapshot,
    input: &Value,
) -> (Option<String>, Vec<String>) {
    let action = text(input, "action").unwrap_or_default();
    if !matches!(action.as_str(), "create" | "update") {
        return (None, Vec::new());
    }
    let state = todo_state(host).await;
    let (event, tasks) = match action.as_str() {
        "create" => ("TaskCreated", created_tasks(input, &state)),
        _ => (
            "TaskCompleted",
            completed_task(input, &state).into_iter().collect(),
        ),
    };
    let mut context = Vec::new();
    for task in tasks {
        let d = Dispatch::new(event, Target::Any, payload(&task)).claude_only();
        let outcomes = dispatch(rt, host, snap, d).await;
        if let Some(reason) = first_block(&outcomes) {
            return (Some(reason), context);
        }
        context.extend(contexts(&outcomes));
    }
    (None, context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_ids_follow_next_id() {
        let state = json!({ "nextId": 4, "tasks": [{ "id": 3, "subject": "old" }] });
        let batch = json!({ "action": "create", "tasks": [{ "subject": "A" }, { "subject": "B", "owner": "w" }] });
        let tasks = created_tasks(&batch, &state);
        assert_eq!(
            tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["4", "5"]
        );
        assert_eq!(tasks[1].owner.as_deref(), Some("w"));
        let single = json!({ "action": "create", "subject": "C", "description": "d" });
        let tasks = created_tasks(&single, &Value::Null);
        assert_eq!(tasks[0].id, "1");
        assert_eq!(tasks[0].description.as_deref(), Some("d"));
    }

    #[test]
    fn completion_reads_the_subject_from_state() {
        let state = json!({ "tasks": [{ "id": 2, "subject": "Ship", "description": "x" }] });
        let done = json!({ "action": "update", "id": 2, "status": "completed" });
        let task = completed_task(&done, &state).unwrap();
        assert_eq!((task.id.as_str(), task.subject.as_str()), ("2", "Ship"));
        let progress = json!({ "action": "update", "id": 2, "status": "in_progress" });
        assert!(completed_task(&progress, &state).is_none());
    }
}
