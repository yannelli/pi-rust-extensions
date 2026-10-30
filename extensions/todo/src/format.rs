//! Tool result text, the `details` envelope, and the `/todos` text listing.

use serde_json::Value;

use crate::graph::blocks_of;
use crate::model::{Task, TaskDetails, TaskStatus, TodoState};
use crate::params::Action;
use crate::reducer::Op;

/// Drops terminal control sequences; newlines and tabs become spaces.
pub fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' | '\u{9b}' | '\u{9d}' => skip_escape(c, &mut chars),
            '\n' | '\r' | '\t' | '\u{2028}' | '\u{2029}' => out.push(' '),
            '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn skip_escape(first: char, chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    let kind = match first {
        '\u{9b}' => '[',
        '\u{9d}' => ']',
        _ => match chars.next() {
            Some(c) => c,
            None => return,
        },
    };
    match kind {
        '[' => {
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
        ']' => {
            while let Some(c) = chars.next() {
                if c == '\x07' || c == '\u{9c}' {
                    break;
                }
                if c == '\x1b' {
                    chars.next_if_eq(&'\\');
                    break;
                }
            }
        }
        _ => {}
    }
}

fn dep_list(ids: &[u64], sep: &str) -> String {
    ids.iter()
        .map(|id| format!("#{id}"))
        .collect::<Vec<_>>()
        .join(sep)
}

fn list_line(t: &Task) -> String {
    let block = if t.blocked_by.is_empty() {
        String::new()
    } else {
        format!(" ⛓ {}", dep_list(&t.blocked_by, ","))
    };
    let form = match (&t.active_form, t.status) {
        (Some(form), TaskStatus::InProgress) => format!(" ({})", sanitize(form)),
        _ => String::new(),
    };
    let (status, id, subject) = (t.status.as_str(), t.id, sanitize(&t.subject));
    format!("[{status}] #{id} {subject}{form}{block}")
}

fn get_lines(task: &Task, state: &TodoState) -> String {
    let mut lines = vec![format!(
        "#{} [{}] {}",
        task.id,
        task.status.as_str(),
        sanitize(&task.subject)
    )];
    if let Some(d) = &task.description {
        lines.push(format!("  description: {}", sanitize(d)));
    }
    if let Some(f) = &task.active_form {
        lines.push(format!("  activeForm: {}", sanitize(f)));
    }
    if !task.blocked_by.is_empty() {
        lines.push(format!("  blockedBy: {}", dep_list(&task.blocked_by, ", ")));
    }
    let blocks = blocks_of(&state.tasks, task.id);
    if !blocks.is_empty() {
        lines.push(format!("  blocks: {}", dep_list(&blocks, ", ")));
    }
    if let Some(o) = &task.owner {
        lines.push(format!("  owner: {}", sanitize(o)));
    }
    if let Some(phase) = task.phase.and_then(|id| state.phase(id)) {
        lines.push(format!("  phase: P{} {}", phase.id, sanitize(&phase.title)));
    }
    lines.join("\n")
}

fn phase_heading(state: &TodoState, id: Option<u64>) -> String {
    match id.and_then(|id| state.phase(id)) {
        None => "── No phase ──".into(),
        Some(p) => {
            let goal = p.goal.as_ref().map(|g| format!(" · goal: {}", sanitize(g)));
            let (id, title, status) = (p.id, sanitize(&p.title), p.status.as_str());
            format!("── P{id} {title} [{status}]{} ──", goal.unwrap_or_default())
        }
    }
}

/// Phase ids in display order, then `None` for tasks without a phase.
pub fn phase_order(state: &TodoState) -> Vec<Option<u64>> {
    let mut order: Vec<Option<u64>> = state.phases.iter().map(|p| Some(p.id)).collect();
    order.push(None);
    order
}

fn list_text(state: &TodoState, status: Option<TaskStatus>, include_deleted: bool) -> String {
    let shown =
        |t: &&Task| (include_deleted || t.is_visible()) && status.is_none_or(|s| t.status == s);
    let view: Vec<&Task> = state.tasks.iter().filter(shown).collect();
    if view.is_empty() {
        return "No tasks".into();
    }
    if state.phases.is_empty() {
        return view
            .iter()
            .map(|t| list_line(t))
            .collect::<Vec<_>>()
            .join("\n");
    }
    let mut lines = Vec::new();
    for phase in phase_order(state) {
        let rows: Vec<String> = view
            .iter()
            .filter(|t| t.phase == phase)
            .map(|t| list_line(t))
            .collect();
        if !rows.is_empty() || phase.is_some() {
            lines.push(phase_heading(state, phase));
            lines.extend(rows);
        }
    }
    lines.join("\n")
}

pub fn content(op: &Op, state: &TodoState) -> String {
    match op {
        Op::Create(ids) => ids
            .iter()
            .map(|id| match state.task(*id) {
                Some(t) => format!("Created #{id}: {} (pending)", sanitize(&t.subject)),
                None => format!("Created #{id}"),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Op::Update {
            id,
            to,
            changed: false,
            ..
        } => format!(
            "No change: #{id} already matches the requested values (status: {})",
            to.as_str()
        ),
        Op::Update { id, from, to, .. } if from != to => {
            format!("Updated #{id} ({} → {})", from.as_str(), to.as_str())
        }
        Op::Update { id, .. } => format!("Updated #{id}"),
        Op::Delete { id, subject } => format!("Deleted #{id}: {}", sanitize(subject)),
        Op::Clear(count) => format!("Cleared {count} tasks"),
        Op::List {
            status,
            include_deleted,
        } => list_text(state, *status, *include_deleted),
        Op::Get(id) => state
            .task(*id)
            .map_or_else(|| format!("#{id} not found"), |t| get_lines(t, state)),
        Op::Move {
            id, changed: false, ..
        } => format!("No change: #{id} is already at that position"),
        Op::Move { id, position, .. } => format!("Moved #{id} to position {position}"),
        Op::AddPhase(id) => state.phase(*id).map_or_else(
            || format!("Added phase P{id}"),
            |p| format!("Added phase P{id}: {} (pending)", sanitize(&p.title)),
        ),
        Op::UpdatePhase {
            id,
            changed: false,
            to,
            ..
        } => format!(
            "No change: phase P{id} already matches the requested values (status: {})",
            to.as_str()
        ),
        Op::UpdatePhase { id, from, to, .. } if from != to => {
            format!("Updated phase P{id} ({} → {})", from.as_str(), to.as_str())
        }
        Op::UpdatePhase { id, .. } => format!("Updated phase P{id}"),
        Op::SetGoal {
            id,
            goal: Some(goal),
            ..
        } => format!("Set goal of phase P{id}: {}", sanitize(goal)),
        Op::SetGoal { id, goal: None, .. } => format!("Cleared goal of phase P{id}"),
        Op::DeletePhase { id, title, moved } => {
            format!(
                "Deleted phase P{id}: {} ({moved} tasks moved to no phase)",
                sanitize(title)
            )
        }
        Op::Error(message) => format!("Error: {message}"),
    }
}

pub fn details(
    action: Action,
    params: Value,
    state: &TodoState,
    op: &Op,
    scope: Option<&str>,
) -> TaskDetails {
    TaskDetails {
        action: action.as_str().into(),
        params,
        state: state.clone(),
        error: match op {
            Op::Error(message) => Some(message.clone()),
            _ => None,
        },
        scope: scope.map(str::to_owned),
    }
}

/// The `/todos` text: a count header, then tasks grouped by status.
pub fn grouped(state: &TodoState) -> Option<String> {
    let visible: Vec<&Task> = state.visible_tasks().collect();
    if visible.is_empty() {
        return None;
    }
    let of = |s: TaskStatus| visible.iter().filter(move |t| t.status == s).copied();
    let (done, active, pending) = (
        of(TaskStatus::Completed).count(),
        of(TaskStatus::InProgress).count(),
        of(TaskStatus::Pending).count(),
    );
    let mut header = Vec::new();
    if done > 0 {
        header.push(format!("{done}/{} completed", visible.len()));
    }
    if active > 0 {
        header.push(format!("{active} in progress"));
    }
    if pending > 0 {
        header.push(format!("{pending} pending"));
    }
    let mut lines = vec![header.join(" · ")];
    let sections = [
        (TaskStatus::Pending, "── Pending ──"),
        (TaskStatus::InProgress, "── In Progress ──"),
        (TaskStatus::Completed, "── Completed ──"),
    ];
    for (status, title) in sections {
        let tasks: Vec<&Task> = of(status).collect();
        if tasks.is_empty() {
            continue;
        }
        lines.push(title.into());
        for t in tasks {
            let form = match (&t.active_form, t.status) {
                (Some(f), TaskStatus::InProgress) => format!(" ({})", sanitize(f)),
                _ => String::new(),
            };
            let block = if t.blocked_by.is_empty() {
                String::new()
            } else {
                format!("    ⛓ {}", dep_list(&t.blocked_by, ","))
            };
            let line = format!(
                "  {} #{} {}{form}{block}",
                status.glyph(),
                t.id,
                sanitize(&t.subject)
            );
            lines.push(line);
        }
    }
    Some(lines.join("\n"))
}
