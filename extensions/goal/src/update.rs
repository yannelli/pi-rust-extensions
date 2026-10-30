use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wait {
    pub reason: String,
    pub deadline: String,
    pub wait_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdatePlan {
    Complete {
        summary: Option<String>,
    },
    Blocked {
        reason: String,
        attempted_actions: Vec<String>,
    },
    Paused {
        reason: String,
        suggested_action: Option<String>,
    },
    Ready,
    Wait(Wait),
}

const KEYS: [&str; 6] = [
    "status",
    "continuation",
    "reason",
    "attempted_actions",
    "suggested_action",
    "completion_summary",
];

/// Port of upstream `planUpdateGoal`: classify an `update_goal` call before changing state.
pub fn plan_update_goal(input: &Value) -> Result<UpdatePlan, String> {
    let object = input
        .as_object()
        .filter(|map| map.keys().all(|key| KEYS.contains(&key.as_str())))
        .ok_or("Provide a lifecycle status or continuation using the declared fields.")?;
    let mut strings: Map<String, Value> = Map::new();
    for key in ["status", "reason", "suggested_action", "completion_summary"] {
        match object.get(key) {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) if s.trim().is_empty() => {}
            Some(Value::String(s)) => {
                strings.insert(key.into(), Value::String(s.clone()));
            }
            Some(_) => return Err(format!("{key} must be a string.")),
        }
    }
    let text = |key: &str| strings.get(key).and_then(Value::as_str).map(str::to_owned);
    let status = text("status");
    if status
        .as_deref()
        .is_some_and(|s| !["complete", "blocked", "paused"].contains(&s))
    {
        return Err("status must be complete, blocked, or paused.".into());
    }
    let attempted = attempted_actions(object.get("attempted_actions"))?;
    let continuation = match object.get("continuation") {
        None | Some(Value::Null) => None,
        Some(raw) => continuation(raw, status.is_some())?,
    };
    if status.is_some() && continuation.is_some() {
        return Err("status conflicts with continuation.wait. Choose one action; a payload-free ready may accompany status as filler.".into());
    }
    let Some(status) = status else {
        return continuation.ok_or_else(|| "Provide one lifecycle status or continuation.".into());
    };
    let allowed: &[&str] = match status.as_str() {
        "complete" => &["completion_summary"],
        "blocked" => &["reason", "attempted_actions"],
        _ => &["reason", "suggested_action"],
    };
    let mut supplied: Vec<&str> = strings
        .keys()
        .map(String::as_str)
        .filter(|key| *key != "status")
        .collect();
    if !attempted.is_empty() {
        supplied.push("attempted_actions");
    }
    let conflicts: Vec<&str> = supplied
        .into_iter()
        .filter(|key| !allowed.contains(key))
        .collect();
    if !conflicts.is_empty() {
        return Err(format!(
            "status={status} conflicts with {}. Use only the fields for that action; empty strings and arrays may remain filler.",
            conflicts.join(", ")
        ));
    }
    let reason = text("reason");
    match status.as_str() {
        "complete" => Ok(UpdatePlan::Complete {
            summary: text("completion_summary"),
        }),
        _ if reason.is_none() => Err(format!(
            "update_goal({{ status: \"{status}\" }}) requires a \"reason\" describing why work cannot continue."
        )),
        "blocked" => Ok(UpdatePlan::Blocked {
            reason: reason.unwrap_or_default(),
            attempted_actions: attempted,
        }),
        _ => Ok(UpdatePlan::Paused {
            reason: reason.unwrap_or_default(),
            suggested_action: text("suggested_action"),
        }),
    }
}

fn attempted_actions(raw: Option<&Value>) -> Result<Vec<String>, String> {
    let error = "attempted_actions must contain at most eight strings of at most 240 characters.";
    let Some(raw) = raw.filter(|v| !v.is_null()) else {
        return Ok(Vec::new());
    };
    let list = raw.as_array().filter(|l| l.len() <= 8).ok_or(error)?;
    let mut out = Vec::new();
    for item in list {
        let text = item
            .as_str()
            .filter(|s| s.chars().count() <= 240)
            .ok_or(error)?;
        if !text.trim().is_empty() {
            out.push(text.to_owned());
        }
    }
    Ok(out)
}

fn continuation(raw: &Value, has_status: bool) -> Result<Option<UpdatePlan>, String> {
    let object = raw
        .as_object()
        .ok_or("continuation must be a ready or wait object.")?;
    let kind = object.get("kind").and_then(Value::as_str);
    if kind == Some("ready") && object.len() == 1 {
        return Ok((!has_status).then_some(UpdatePlan::Ready));
    }
    let wait_keys = ["kind", "reason", "deadline", "wait_id", "polling"];
    if kind != Some("wait") || object.keys().any(|key| !wait_keys.contains(&key.as_str())) {
        return Err("continuation must be payload-free ready or a valid wait; unexpected continuation fields are not filler.".into());
    }
    let reason = object.get("reason").and_then(Value::as_str);
    let deadline = object.get("deadline").and_then(Value::as_str);
    let wait_id = object.get("wait_id");
    let (Some(reason), Some(deadline)) = (reason, deadline) else {
        return Err(wait_error());
    };
    if reason.trim().is_empty() || reason.len() > 2000 || wait_id.is_some_and(|v| !v.is_string()) {
        return Err(wait_error());
    }
    Ok(Some(UpdatePlan::Wait(Wait {
        reason: reason.to_owned(),
        deadline: deadline.to_owned(),
        wait_id: wait_id.and_then(Value::as_str).map(str::to_owned),
    })))
}

fn wait_error() -> String {
    "continuation.wait requires reason and deadline strings; wait_id must be a string.".into()
}
