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
            "subject": { "type": "string", "description": "Short imperative task" },
            "description": { "type": "string" },
            "activeForm": { "type": "string", "description": "Present-continuous spinner label" },
            "blockedBy": ids("Existing prerequisite ids"),
            "owner": { "type": "string" },
            "metadata": { "type": "object", "additionalProperties": true },
            "phase": { "type": "number" }
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
            "subject": { "type": "string", "description": "Short imperative task; create requires this or tasks" },
            "description": { "type": "string" },
            "activeForm": { "type": "string", "description": "Present-continuous spinner label" },
            "status": { "type": "string", "enum": ["pending", "in_progress", "completed", "deleted"],
                "description": "update sets; list filters; update_phase excludes deleted" },
            "blockedBy": ids("create: prerequisite ids"),
            "addBlockedBy": ids("update: add dependencies"),
            "removeBlockedBy": ids("update: remove dependencies"),
            "owner": { "type": "string" },
            "metadata": { "type": "object", "additionalProperties": true, "description": "Null values delete keys on update" },
            "id": { "type": "number", "description": "Required: update/get/delete/move" },
            "includeDeleted": { "type": "boolean", "default": false },
            "tasks": { "type": "array", "items": create_item_schema(),
                "description": "Atomic create batch; omit subject; existing dependencies only" },
            "phase": { "type": "number", "description": "Phase id; create batch default; update: 0 unassigns" },
            "title": { "type": "string", "description": "add_phase/update_phase title" },
            "goal": { "type": "string", "description": "add_phase/update_phase/set_goal; empty clears" },
            "position": { "type": "number", "description": "1-based task/phase order" }
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
