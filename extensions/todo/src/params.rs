//! `todo` tool parameters: lenient parsing from JSON and the JSON Schema.

use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Create,
    Update,
    List,
    Get,
    Delete,
    Clear,
    Move,
    AddPhase,
    UpdatePhase,
    SetGoal,
    DeletePhase,
}

pub const ACTIONS: &[(&str, Action)] = &[
    ("create", Action::Create),
    ("update", Action::Update),
    ("list", Action::List),
    ("get", Action::Get),
    ("delete", Action::Delete),
    ("clear", Action::Clear),
    ("move", Action::Move),
    ("add_phase", Action::AddPhase),
    ("update_phase", Action::UpdatePhase),
    ("set_goal", Action::SetGoal),
    ("delete_phase", Action::DeletePhase),
];

impl Action {
    pub fn as_str(self) -> &'static str {
        ACTIONS
            .iter()
            .find(|(_, a)| *a == self)
            .map_or("", |(n, _)| n)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Params {
    pub subject: Option<String>,
    pub description: Option<String>,
    pub active_form: Option<String>,
    pub status: Option<String>,
    pub blocked_by: Option<Vec<u64>>,
    pub add_blocked_by: Option<Vec<u64>>,
    pub remove_blocked_by: Option<Vec<u64>>,
    pub owner: Option<String>,
    pub metadata: Option<Map<String, Value>>,
    pub id: Option<u64>,
    pub include_deleted: bool,
    pub tasks: Option<Vec<Value>>,
    pub phase: Option<u64>,
    pub title: Option<String>,
    pub goal: Option<String>,
    pub position: Option<u64>,
}

fn present<'a>(obj: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    obj.get(key).filter(|v| !v.is_null())
}

fn text(obj: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match present(obj, key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.replace("\r\n", "\n"))),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

fn id_of(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|f| f.fract() == 0.0 && *f >= 0.0)
            .map(|f| f as u64)
    })
}

fn number(obj: &Map<String, Value>, key: &str) -> Result<Option<u64>, String> {
    match present(obj, key) {
        None => Ok(None),
        Some(v) => id_of(v)
            .map(Some)
            .ok_or_else(|| format!("{key} must be a non-negative integer")),
    }
}

fn numbers(obj: &Map<String, Value>, key: &str) -> Result<Option<Vec<u64>>, String> {
    match present(obj, key) {
        None => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| id_of(v).ok_or_else(|| format!("{key} must be an array of task ids")))
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(format!("{key} must be an array of task ids")),
    }
}

/// Undoes a double-encoded `tasks[]` (a JSON string at an array or item position).
pub fn prepare(mut args: Value) -> Value {
    let Some(obj) = args.as_object_mut() else {
        return args;
    };
    if let Some(Value::String(raw)) = obj.get("tasks")
        && let Ok(parsed @ Value::Array(_)) = serde_json::from_str::<Value>(raw)
    {
        obj.insert("tasks".into(), parsed);
    }
    if let Some(Value::Array(items)) = obj.get_mut("tasks") {
        for item in items.iter_mut() {
            if let Value::String(raw) = item
                && let Ok(parsed @ Value::Object(_)) = serde_json::from_str::<Value>(raw)
            {
                *item = parsed;
            }
        }
    }
    args
}

pub fn parse_action(args: &Value) -> Result<Action, String> {
    let name = args.get("action").and_then(Value::as_str).unwrap_or("");
    ACTIONS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, a)| *a)
        .ok_or_else(|| format!("unknown action \"{name}\""))
}

/// Parses an object of `todo` parameters (top level, or one `tasks[]` item).
pub fn parse(args: &Value) -> Result<Params, String> {
    let empty = Map::new();
    let obj = args.as_object().unwrap_or(&empty);
    let metadata = match present(obj, "metadata") {
        None => None,
        Some(Value::Object(map)) => Some(map.clone()),
        Some(_) => return Err("metadata must be an object".into()),
    };
    let tasks = match present(obj, "tasks") {
        None => None,
        Some(Value::Array(items)) => Some(items.clone()),
        Some(_) => return Err("tasks must be an array".into()),
    };
    Ok(Params {
        subject: text(obj, "subject")?,
        description: text(obj, "description")?,
        active_form: text(obj, "activeForm")?,
        status: text(obj, "status")?,
        blocked_by: numbers(obj, "blockedBy")?,
        add_blocked_by: numbers(obj, "addBlockedBy")?,
        remove_blocked_by: numbers(obj, "removeBlockedBy")?,
        owner: text(obj, "owner")?,
        metadata,
        id: number(obj, "id")?,
        include_deleted: present(obj, "includeDeleted").and_then(Value::as_bool) == Some(true),
        tasks,
        phase: number(obj, "phase")?,
        title: text(obj, "title")?,
        goal: text(obj, "goal")?,
        position: number(obj, "position")?,
    })
}

fn ids(description: &str) -> Value {
    json!({ "type": "array", "items": { "type": "number" }, "description": description })
}

fn create_item_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "subject": { "type": "string", "description": "Task subject line" },
            "description": { "type": "string", "description": "Long-form task description" },
            "activeForm": { "type": "string", "description": "Present-continuous spinner label shown while status is in_progress (e.g. 'writing tests')" },
            "blockedBy": ids("Existing task ids this item waits on. Cannot name another item in this same batch; create the prerequisite first, then the dependent with blockedBy."),
            "owner": { "type": "string", "description": "Agent/owner assigned to this task" },
            "metadata": { "type": "object", "additionalProperties": true, "description": "Arbitrary metadata" },
            "phase": { "type": "number", "description": "Phase id this task belongs to" }
        },
        "required": ["subject"]
    })
}

pub fn schema() -> Value {
    let actions: Vec<&str> = ACTIONS.iter().map(|(n, _)| *n).collect();
    json!({
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": actions },
            "subject": { "type": "string", "description": "Task subject line (required for create unless tasks[] is set)" },
            "description": { "type": "string", "description": "Long-form task description" },
            "activeForm": { "type": "string", "description": "Present-continuous spinner label shown while status is in_progress (e.g. 'writing tests')" },
            "status": { "type": "string", "enum": ["pending", "in_progress", "completed", "deleted"],
                "description": "Set this task's status (update): one of pending, in_progress, completed, deleted. When action is list, filters returned tasks by this status. For update_phase: pending, in_progress, or completed." },
            "blockedBy": ids("Initial blockedBy ids (create only)"),
            "addBlockedBy": ids("Task ids to add to blockedBy (update only, additive merge)"),
            "removeBlockedBy": ids("Task ids to remove from blockedBy (update only, additive merge)"),
            "owner": { "type": "string", "description": "Agent/owner assigned to this task" },
            "metadata": { "type": "object", "additionalProperties": true, "description": "Arbitrary metadata; pass null value for a key to delete that key on update" },
            "id": { "type": "number", "description": "Task id (required for update, get, delete, move)" },
            "includeDeleted": { "type": "boolean", "description": "If true, list action returns deleted (tombstoned) tasks as well. Default: false." },
            "tasks": { "type": "array", "items": create_item_schema(),
                "description": "Batch create: independent tasks in one call, one-of with subject. The whole list is validated then committed together. Items cannot blockedBy each other; create the prerequisite first, then the dependent with blockedBy." },
            "phase": { "type": "number", "description": "Phase id: the phase of a created task (create, also the default for tasks[] items), the new phase of a task (update), or the phase to change (update_phase, set_goal, delete_phase)" },
            "title": { "type": "string", "description": "Phase title (add_phase, update_phase)" },
            "goal": { "type": "string", "description": "Phase goal (add_phase, update_phase, set_goal); an empty string clears it" },
            "position": { "type": "number", "description": "1-based position to move the task (move) or phase (update_phase) to" }
        },
        "required": ["action"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_integral_floats_and_skips_nulls() {
        let params = parse(&json!({ "id": 3.0, "owner": null, "blockedBy": [1, 2.0] })).unwrap();
        assert_eq!(params.id, Some(3));
        assert_eq!(params.owner, None);
        assert_eq!(params.blocked_by, Some(vec![1, 2]));
        assert!(parse(&json!({ "id": "x" })).is_err());
    }

    #[test]
    fn prepare_decodes_double_encoded_tasks() {
        let args = json!({ "action": "create", "tasks": "[\"{\\\"subject\\\":\\\"A\\\"}\"]" });
        let prepared = prepare(args);
        assert_eq!(prepared["tasks"], json!([{ "subject": "A" }]));
    }

    #[test]
    fn unknown_action_is_rejected() {
        assert_eq!(parse_action(&json!({ "action": "list" })), Ok(Action::List));
        assert!(parse_action(&json!({ "action": "zap" })).is_err());
    }
}
