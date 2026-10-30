//! Pure reducer for the todo actions: `(state, action, params) -> (state, op)`.

use crate::model::{PhaseStatus, Task, TaskStatus, TodoState};
use crate::params::{Action, Params, parse};
use crate::phases;
use crate::update::update;

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Create(Vec<u64>),
    Update {
        id: u64,
        from: TaskStatus,
        to: TaskStatus,
        changed: bool,
    },
    Delete {
        id: u64,
        subject: String,
    },
    List {
        status: Option<TaskStatus>,
        include_deleted: bool,
    },
    Get(u64),
    Clear(usize),
    Move {
        id: u64,
        position: usize,
        changed: bool,
    },
    AddPhase(u64),
    UpdatePhase {
        id: u64,
        from: PhaseStatus,
        to: PhaseStatus,
        changed: bool,
    },
    SetGoal {
        id: u64,
        goal: Option<String>,
        changed: bool,
    },
    DeletePhase {
        id: u64,
        title: String,
        moved: usize,
    },
    Error(String),
}

impl Op {
    /// True when the op changed state and its snapshot must be persisted.
    pub fn changes_state(&self) -> bool {
        match self {
            Op::Create(_) | Op::Delete { .. } | Op::Clear(_) => true,
            Op::AddPhase(_) | Op::DeletePhase { .. } => true,
            Op::Update { changed, .. }
            | Op::Move { changed, .. }
            | Op::UpdatePhase { changed, .. }
            | Op::SetGoal { changed, .. } => *changed,
            Op::List { .. } | Op::Get(_) | Op::Error(_) => false,
        }
    }
}

pub type Outcome = (TodoState, Op);

pub fn error(state: &TodoState, message: impl Into<String>) -> Outcome {
    (state.clone(), Op::Error(message.into()))
}

type FieldCheck = (&'static str, fn(&Params) -> bool);

const BATCH_ITEM_FIELDS: &[FieldCheck] = &[
    ("description", |p| p.description.is_some()),
    ("activeForm", |p| p.active_form.is_some()),
    ("blockedBy", |p| p.blocked_by.is_some()),
    ("owner", |p| p.owner.is_some()),
    ("metadata", |p| p.metadata.is_some()),
];

pub fn apply(state: &TodoState, action: Action, params: &Params) -> Outcome {
    match action {
        Action::Create => create(state, params),
        Action::Update => update(state, params),
        Action::List => list(state, params),
        Action::Get => match params.id {
            None => error(state, "id required for get"),
            Some(id) if state.task(id).is_none() => error(state, format!("#{id} not found")),
            Some(id) => (state.clone(), Op::Get(id)),
        },
        Action::Delete => delete(state, params),
        Action::Clear => (TodoState::default(), Op::Clear(state.tasks.len())),
        Action::Move => move_task(state, params),
        Action::AddPhase => phases::add(state, params),
        Action::UpdatePhase => phases::update(state, params),
        Action::SetGoal => phases::set_goal(state, params),
        Action::DeletePhase => phases::delete(state, params),
    }
}

fn check_blocked_by(state: &TodoState, deps: &[u64], siblings: &[u64]) -> Result<(), String> {
    for dep in deps {
        if siblings.contains(dep) {
            return Err(format!(
                "blockedBy: #{dep} is another item in this batch; create the prerequisite first, then the dependent with blockedBy"
            ));
        }
        match state.task(*dep) {
            None => return Err(format!("blockedBy: #{dep} not found")),
            Some(t) if t.status == TaskStatus::Deleted => {
                return Err(format!("blockedBy: #{dep} is deleted"));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// `None` and `0` mean no phase.
pub fn check_phase(state: &TodoState, phase: Option<u64>) -> Result<Option<u64>, String> {
    match phase {
        None | Some(0) => Ok(None),
        Some(id) if state.phase(id).is_some() => Ok(Some(id)),
        Some(id) => Err(format!("phase: P{id} not found")),
    }
}

fn build(id: u64, spec: &Params, subject: &str, phase: Option<u64>) -> Task {
    let mut task = Task::new(id, subject);
    task.description = spec.description.clone().filter(|s| !s.is_empty());
    task.active_form = spec.active_form.clone().filter(|s| !s.is_empty());
    task.blocked_by = spec.blocked_by.clone().unwrap_or_default();
    task.owner = spec.owner.clone().filter(|s| !s.is_empty());
    task.metadata = spec.metadata.clone();
    task.phase = phase;
    task
}

fn create(state: &TodoState, params: &Params) -> Outcome {
    let Some(batch) = &params.tasks else {
        let Some(subject) = params.subject.as_deref().filter(|s| !s.trim().is_empty()) else {
            return error(state, "subject required for create");
        };
        let checked =
            check_blocked_by(state, params.blocked_by.as_deref().unwrap_or_default(), &[])
                .and_then(|()| check_phase(state, params.phase));
        let phase = match checked {
            Ok(phase) => phase,
            Err(message) => return error(state, message),
        };
        let mut next = state.clone();
        next.tasks
            .push(build(state.next_id, params, subject, phase));
        next.next_id += 1;
        return (next, Op::Create(vec![state.next_id]));
    };
    if params.subject.is_some() {
        return error(state, "create requires subject or tasks[], not both");
    }
    let top: Vec<&str> = BATCH_ITEM_FIELDS
        .iter()
        .filter(|(_, set)| set(params))
        .map(|(name, _)| *name)
        .collect();
    if !top.is_empty() {
        let fields = top.join(", ");
        return error(
            state,
            format!("create with tasks[] takes {fields} on each item, not at the top level"),
        );
    }
    if batch.is_empty() {
        return error(state, "tasks[] must be non-empty");
    }
    let default_phase = match check_phase(state, params.phase) {
        Ok(phase) => phase,
        Err(message) => return error(state, message),
    };
    let mut items = Vec::new();
    for (i, raw) in batch.iter().enumerate() {
        let spec = match raw.is_object().then(|| parse(raw)) {
            Some(Ok(spec)) => spec,
            Some(Err(message)) => return error(state, format!("tasks[{i}]: {message}")),
            None => return error(state, format!("tasks[{i}]: subject required for create")),
        };
        let Some(subject) = spec.subject.clone().filter(|s| !s.trim().is_empty()) else {
            return error(state, format!("tasks[{i}]: subject required for create"));
        };
        let phase = match spec.phase {
            Some(_) => check_phase(state, spec.phase),
            None => Ok(default_phase),
        };
        match phase {
            Ok(phase) => items.push((spec, subject, phase)),
            Err(message) => return error(state, format!("tasks[{i}]: {message}")),
        }
    }
    let siblings: Vec<u64> = (0..items.len() as u64).map(|i| state.next_id + i).collect();
    for (spec, _, _) in &items {
        let deps = spec.blocked_by.as_deref().unwrap_or_default();
        if let Err(message) = check_blocked_by(state, deps, &siblings) {
            return error(state, message);
        }
    }
    let mut next = state.clone();
    for ((spec, subject, phase), id) in items.iter().zip(&siblings) {
        next.tasks.push(build(*id, spec, subject, *phase));
    }
    next.next_id += siblings.len() as u64;
    (next, Op::Create(siblings))
}

fn list(state: &TodoState, params: &Params) -> Outcome {
    let status = match &params.status {
        None => None,
        Some(raw) => match TaskStatus::parse(raw) {
            Some(status) => Some(status),
            None => return error(state, format!("invalid status \"{raw}\"")),
        },
    };
    let include_deleted = params.include_deleted;
    (
        state.clone(),
        Op::List {
            status,
            include_deleted,
        },
    )
}

fn delete(state: &TodoState, params: &Params) -> Outcome {
    let Some(id) = params.id else {
        return error(state, "id required for delete");
    };
    let Some(index) = state.tasks.iter().position(|t| t.id == id) else {
        return error(state, format!("#{id} not found"));
    };
    if state.tasks[index].status == TaskStatus::Deleted {
        return error(state, format!("#{id} is already deleted"));
    }
    let mut next = state.clone();
    next.tasks[index].status = TaskStatus::Deleted;
    let subject = next.tasks[index].subject.clone();
    (next, Op::Delete { id, subject })
}

fn move_task(state: &TodoState, params: &Params) -> Outcome {
    let Some(id) = params.id else {
        return error(state, "id required for move");
    };
    let Some(index) = state.tasks.iter().position(|t| t.id == id) else {
        return error(state, format!("#{id} not found"));
    };
    let Some(position) = params.position.filter(|p| *p >= 1) else {
        return error(state, "position required for move (1-based)");
    };
    let target = (position as usize - 1).min(state.tasks.len() - 1);
    let mut next = state.clone();
    let task = next.tasks.remove(index);
    next.tasks.insert(target, task);
    let changed = target != index;
    (
        next,
        Op::Move {
            id,
            position: target + 1,
            changed,
        },
    )
}
