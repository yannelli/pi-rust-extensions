use std::collections::HashSet;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::state::{Goal, GoalTask, TaskStatus};

pub const MAX_TASKS: usize = 200;
pub const MAX_EVIDENCE: usize = 200;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FlatTask {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub verification_contract: Option<String>,
    #[serde(default)]
    pub lightweight_subtasks: Option<bool>,
}

/// Validates a flat parent-linked list and merges it with `existing`: matching ids keep progress.
pub fn merge_tasks(input: &[FlatTask], existing: &[GoalTask]) -> Result<Vec<GoalTask>, String> {
    if input.len() > MAX_TASKS {
        return Err(format!("At most {MAX_TASKS} tasks are allowed."));
    }
    let mut ids = HashSet::new();
    for task in input {
        let id = task.id.trim();
        if id.is_empty() || task.title.trim().is_empty() {
            return Err("Every task needs a non-empty id and title.".into());
        }
        if !ids.insert(id) {
            return Err(format!("Duplicate task id \"{id}\"."));
        }
    }
    let parent_of = |id: &str| {
        input
            .iter()
            .find(|task| task.id.trim() == id)
            .and_then(|task| clean(task.parent_id.as_deref()))
    };
    for task in input {
        let mut seen = HashSet::from([task.id.trim().to_owned()]);
        let mut parent = clean(task.parent_id.as_deref());
        while let Some(id) = parent {
            if !ids.contains(id.as_str()) {
                return Err(format!(
                    "Task \"{}\" names unknown parent \"{id}\".",
                    task.id.trim()
                ));
            }
            if !seen.insert(id.clone()) {
                return Err(format!(
                    "Task \"{}\" is part of a parent cycle.",
                    task.id.trim()
                ));
            }
            parent = parent_of(&id);
        }
    }
    Ok(input
        .iter()
        .map(|task| {
            let id = task.id.trim().to_owned();
            let old = existing.iter().find(|old| old.id == id);
            GoalTask {
                title: task.title.trim().to_owned(),
                parent_id: clean(task.parent_id.as_deref()),
                verification_contract: clean(task.verification_contract.as_deref()),
                lightweight_subtasks: task.lightweight_subtasks.unwrap_or(false),
                status: old.map_or(TaskStatus::Pending, |old| old.status),
                evidence: old.and_then(|old| old.evidence.clone()),
                skip_reason: old.and_then(|old| old.skip_reason.clone()),
                id,
            }
        })
        .collect())
}

fn clean(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskProgress {
    pub task_id: String,
    pub status: String,
    pub evidence: Option<String>,
    pub reason: Option<String>,
}

const FIELDS: [&str; 4] = ["task_id", "status", "evidence", "reason"];
const STATUSES: [&str; 4] = ["start", "complete", "skipped", "pending"];

fn normalize(raw: &Map<String, Value>) -> Option<Map<String, Value>> {
    let mut out = Map::new();
    for field in FIELDS {
        match raw.get(field) {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) if s.trim().is_empty() => {}
            Some(Value::String(s)) => {
                out.insert(field.into(), Value::String(s.trim().into()));
            }
            Some(_) => return None,
        }
    }
    let status_ok = out
        .get("status")
        .and_then(Value::as_str)
        .is_none_or(|s| STATUSES.contains(&s));
    status_ok.then_some(out)
}

fn complete(map: &Map<String, Value>) -> Option<TaskProgress> {
    let text = |key: &str| map.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(TaskProgress {
        task_id: text("task_id")?,
        status: text("status")?,
        evidence: text("evidence"),
        reason: text("reason"),
    })
}

/// Port of upstream `normalizeTaskProgress`: one task, or an ordered `updates` batch.
pub fn normalize_task_progress(input: &Value) -> Result<Vec<TaskProgress>, String> {
    let Some(object) = input.as_object() else {
        return Err("Provide task_id and status, or an updates batch.".into());
    };
    let root = normalize(object).ok_or(
        "Task fields must be strings; status must be start, complete, skipped, or pending.",
    )?;
    let updates = match object.get("updates") {
        None | Some(Value::Null) => None,
        Some(Value::Array(list)) if list.is_empty() => None,
        Some(other) => Some(other),
    };
    let Some(updates) = updates else {
        return complete(&root)
            .map(|one| vec![one])
            .ok_or_else(|| "Provide task_id and status, or a non-empty updates batch.".into());
    };
    let batch_error = || "updates must contain 1–100 valid task updates.".to_owned();
    let list = updates
        .as_array()
        .filter(|list| list.len() <= 100)
        .ok_or_else(batch_error)?;
    let mut out = Vec::new();
    for entry in list {
        let map = entry.as_object().ok_or_else(batch_error)?;
        if map.keys().any(|key| !FIELDS.contains(&key.as_str())) {
            return Err(batch_error());
        }
        let normalized = normalize(map).ok_or_else(batch_error)?;
        out.push(complete(&normalized).ok_or_else(batch_error)?);
    }
    let matches_root = |entry: &TaskProgress| {
        let value = |key: &str| match key {
            "task_id" => Some(entry.task_id.as_str()),
            "status" => Some(entry.status.as_str()),
            "evidence" => entry.evidence.as_deref(),
            _ => entry.reason.as_deref(),
        };
        FIELDS
            .iter()
            .all(|key| root.get(*key).is_none_or(|v| v.as_str() == value(key)))
    };
    if !out.iter().any(matches_root) {
        return Err("Top-level task fields conflict with updates. Put each instruction in updates and omit the top-level task fields.".into());
    }
    Ok(out)
}

/// Applies all updates or none.
pub fn apply_progress(goal: &mut Goal, updates: &[TaskProgress]) -> Result<Vec<String>, String> {
    let mut draft = goal.clone();
    let mut notes = Vec::new();
    for update in updates {
        let index = draft
            .tasks
            .iter()
            .position(|task| task.id == update.task_id)
            .ok_or_else(|| format!("Unknown task id \"{}\".", update.task_id))?;
        let id = update.task_id.clone();
        match update.status.as_str() {
            "start" => {
                draft.tasks[index].status = TaskStatus::InProgress;
                draft.current_task_id = Some(id.clone());
            }
            "complete" => {
                let evidence = update.evidence.clone();
                if evidence
                    .as_ref()
                    .is_some_and(|e| e.chars().count() > MAX_EVIDENCE)
                {
                    return Err(format!(
                        "evidence for \"{id}\" exceeds {MAX_EVIDENCE} characters."
                    ));
                }
                let task = &draft.tasks[index];
                if !task.lightweight_subtasks
                    && draft
                        .children(Some(&id))
                        .any(|child| !child.status.is_resolved())
                {
                    return Err(format!("Task \"{id}\" has unresolved subtasks."));
                }
                let task = &mut draft.tasks[index];
                task.status = TaskStatus::Complete;
                task.evidence = evidence;
                task.skip_reason = None;
            }
            "skipped" => {
                let reason = update
                    .reason
                    .clone()
                    .ok_or_else(|| format!("Skipping \"{id}\" requires a reason."))?;
                let task = &mut draft.tasks[index];
                task.status = TaskStatus::Skipped;
                task.skip_reason = Some(reason);
            }
            _ => {
                let task = &mut draft.tasks[index];
                task.status = TaskStatus::Pending;
                task.evidence = None;
                task.skip_reason = None;
            }
        }
        notes.push(format!("{id}: {}", update.status));
    }
    let current_resolved = draft
        .current_task_id
        .as_deref()
        .and_then(|id| draft.task(id))
        .is_none_or(|task| task.status != TaskStatus::InProgress);
    if current_resolved {
        draft.current_task_id = None;
    }
    *goal = draft;
    Ok(notes)
}
