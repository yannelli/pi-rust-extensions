//! The `update` action: status transitions, fields, dependencies, metadata, and phase.

use serde_json::Value;

use crate::graph::detect_cycle;
use crate::model::TaskStatus;
use crate::model::TodoState;
use crate::params::Params;
use crate::phases;
use crate::reducer::{Op, Outcome, check_phase, error};

const NO_MUTATION: &str = "update requires at least one mutable field: subject, description, activeForm, status, owner, metadata, addBlockedBy, or removeBlockedBy";

fn has_mutation(params: &Params) -> bool {
    let non_empty = |list: &Option<Vec<u64>>| list.as_ref().is_some_and(|l| !l.is_empty());
    params.subject.is_some()
        || params.description.is_some()
        || params.active_form.is_some()
        || params.status.is_some()
        || params.owner.is_some()
        || params.metadata.is_some()
        || params.phase.is_some()
        || non_empty(&params.add_blocked_by)
        || non_empty(&params.remove_blocked_by)
}

fn add_deps(state: &TodoState, id: u64, add: &[u64], deps: &mut Vec<u64>) -> Result<(), String> {
    for dep in add {
        if *dep == id {
            return Err(format!("cannot block #{id} on itself"));
        }
        match state.task(*dep) {
            None => return Err(format!("addBlockedBy: #{dep} not found")),
            Some(t) if t.status == TaskStatus::Deleted => {
                return Err(format!("addBlockedBy: #{dep} is deleted"));
            }
            Some(_) if !deps.contains(dep) => deps.push(*dep),
            Some(_) => {}
        }
    }
    if detect_cycle(&state.tasks, id, deps) {
        return Err("addBlockedBy would create a cycle in the blockedBy graph".into());
    }
    Ok(())
}

pub fn update(state: &TodoState, params: &Params) -> Outcome {
    let Some(id) = params.id else {
        return error(state, "id required for update");
    };
    let Some(index) = state.tasks.iter().position(|t| t.id == id) else {
        return error(state, format!("#{id} not found"));
    };
    let current = &state.tasks[index];
    if !has_mutation(params) {
        return error(state, NO_MUTATION);
    }
    let mut status = current.status;
    if let Some(raw) = &params.status {
        let Some(target) = TaskStatus::parse(raw) else {
            return error(state, format!("invalid status \"{raw}\""));
        };
        if !current.status.can_become(target) {
            let (from, to) = (current.status.as_str(), target.as_str());
            return error(state, format!("illegal transition {from} → {to}"));
        }
        status = target;
    }
    let mut blocked_by = current.blocked_by.clone();
    if let Some(remove) = &params.remove_blocked_by {
        blocked_by.retain(|dep| !remove.contains(dep));
    }
    if let Some(add) = params.add_blocked_by.as_ref().filter(|l| !l.is_empty())
        && let Err(message) = add_deps(state, id, add, &mut blocked_by)
    {
        return error(state, message);
    }
    let phase = match params.phase {
        None => current.phase,
        some => match check_phase(state, some) {
            Ok(phase) => phase,
            Err(message) => return error(state, message),
        },
    };
    let mut updated = current.clone();
    updated.status = status;
    updated.blocked_by = blocked_by;
    updated.phase = phase;
    if let Some(patch) = &params.metadata {
        let mut merged = current.metadata.clone().unwrap_or_default();
        for (key, value) in patch {
            match value {
                Value::Null => merged.remove(key),
                v => merged.insert(key.clone(), v.clone()),
            };
        }
        updated.metadata = (!merged.is_empty()).then_some(merged);
    }
    if let Some(subject) = &params.subject {
        updated.subject = subject.clone();
    }
    for (field, value) in [
        (&mut updated.description, &params.description),
        (&mut updated.active_form, &params.active_form),
        (&mut updated.owner, &params.owner),
    ] {
        if let Some(v) = value {
            *field = Some(v.clone());
        }
    }
    let changed = updated != *current;
    let mut next = state.clone();
    next.tasks[index] = updated;
    if status == TaskStatus::InProgress {
        phases::start_phase_of(&mut next, id);
    }
    let op = Op::Update {
        id,
        from: current.status,
        to: status,
        changed,
    };
    (next, op)
}
