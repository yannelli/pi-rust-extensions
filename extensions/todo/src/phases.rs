//! Phase actions: `add_phase`, `update_phase`, `set_goal`, `delete_phase`.

use crate::model::{Phase, PhaseStatus, TodoState};
use crate::params::Params;
use crate::reducer::{Op, Outcome, error};

fn index_of(state: &TodoState, params: &Params, action: &str) -> Result<usize, String> {
    let id = params
        .phase
        .ok_or_else(|| format!("phase required for {action}"))?;
    state
        .phases
        .iter()
        .position(|p| p.id == id)
        .ok_or_else(|| format!("phase: P{id} not found"))
}

fn goal_of(raw: &Option<String>) -> Option<String> {
    raw.as_ref().filter(|g| !g.trim().is_empty()).cloned()
}

fn place(phases: &mut Vec<Phase>, index: usize, position: u64) -> usize {
    let target = (position.max(1) as usize - 1).min(phases.len() - 1);
    let phase = phases.remove(index);
    phases.insert(target, phase);
    target
}

pub fn add(state: &TodoState, params: &Params) -> Outcome {
    let Some(title) = params.title.clone().filter(|t| !t.trim().is_empty()) else {
        return error(state, "title required for add_phase");
    };
    let mut next = state.clone();
    let id = state.next_phase_id;
    next.phases.push(Phase {
        id,
        title,
        goal: goal_of(&params.goal),
        status: PhaseStatus::Pending,
    });
    next.next_phase_id += 1;
    if let Some(position) = params.position {
        let last = next.phases.len() - 1;
        place(&mut next.phases, last, position);
    }
    (next, Op::AddPhase(id))
}

pub fn update(state: &TodoState, params: &Params) -> Outcome {
    let index = match index_of(state, params, "update_phase") {
        Ok(index) => index,
        Err(message) => return error(state, message),
    };
    let current = &state.phases[index];
    if params.title.is_none()
        && params.goal.is_none()
        && params.status.is_none()
        && params.position.is_none()
    {
        return error(
            state,
            "update_phase requires at least one of: title, goal, status, position",
        );
    }
    let mut updated = current.clone();
    if let Some(raw) = &params.status {
        match PhaseStatus::parse(raw) {
            Some(status) => updated.status = status,
            None => return error(state, format!("invalid phase status \"{raw}\"")),
        }
    }
    if let Some(title) = params.title.as_ref().filter(|t| !t.trim().is_empty()) {
        updated.title = title.clone();
    }
    if params.goal.is_some() {
        updated.goal = goal_of(&params.goal);
    }
    let mut next = state.clone();
    next.phases[index] = updated.clone();
    let moved = params
        .position
        .is_some_and(|position| place(&mut next.phases, index, position) != index);
    let changed = moved || updated != *current;
    let (from, to) = (current.status, updated.status);
    (
        next,
        Op::UpdatePhase {
            id: current.id,
            from,
            to,
            changed,
        },
    )
}

pub fn set_goal(state: &TodoState, params: &Params) -> Outcome {
    let index = match index_of(state, params, "set_goal") {
        Ok(index) => index,
        Err(message) => return error(state, message),
    };
    let goal = goal_of(&params.goal);
    let mut next = state.clone();
    let changed = next.phases[index].goal != goal;
    next.phases[index].goal = goal.clone();
    let id = next.phases[index].id;
    (next, Op::SetGoal { id, goal, changed })
}

pub fn delete(state: &TodoState, params: &Params) -> Outcome {
    let index = match index_of(state, params, "delete_phase") {
        Ok(index) => index,
        Err(message) => return error(state, message),
    };
    let mut next = state.clone();
    let phase = next.phases.remove(index);
    let mut moved = 0;
    for task in next.tasks.iter_mut().filter(|t| t.phase == Some(phase.id)) {
        task.phase = None;
        moved += 1;
    }
    (
        next,
        Op::DeletePhase {
            id: phase.id,
            title: phase.title,
            moved,
        },
    )
}

/// A task that starts work also starts its pending phase.
pub fn start_phase_of(state: &mut TodoState, task_id: u64) {
    let Some(phase_id) = state.task(task_id).and_then(|t| t.phase) else {
        return;
    };
    if let Some(phase) = state.phases.iter_mut().find(|p| p.id == phase_id)
        && phase.status == PhaseStatus::Pending
    {
        phase.status = PhaseStatus::InProgress;
    }
}
