//! Rows and lines of the `/todos` panel.

use pi_ext_sdk::style::{self, Color};

use crate::format::sanitize;
use crate::model::{Task, TodoState};
use crate::scopes::Visibility;
use crate::store::Runtime;
use crate::widget::task_line;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Phase(u64),
    NoPhase,
    Task(u64),
    Scope(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    AddTask(Option<u64>),
    AddPhase,
    EditTask(u64),
    EditPhase(u64),
    Goal(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Input { purpose: Purpose, buffer: String },
    ConfirmDelete(Row),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelState {
    pub cursor: usize,
    pub mode: Mode,
    pub message: String,
}

impl Default for PanelState {
    fn default() -> Self {
        Self {
            cursor: 0,
            mode: Mode::Normal,
            message: String::new(),
        }
    }
}

const HELP: &str = "↑↓ move · enter status · a add · p phase · e edit · g goal · d delete · K/J reorder · v visibility · c collapse · q close";

fn tasks_in(state: &TodoState, phase: Option<u64>) -> impl Iterator<Item = &Task> {
    state.visible_tasks().filter(move |t| t.phase == phase)
}

/// Selectable rows in display order.
pub fn rows(runtime: &Runtime) -> Vec<Row> {
    let state = &runtime.state;
    let mut rows = Vec::new();
    for phase in &state.phases {
        rows.push(Row::Phase(phase.id));
        rows.extend(tasks_in(state, Some(phase.id)).map(|t| Row::Task(t.id)));
    }
    let loose: Vec<Row> = tasks_in(state, None).map(|t| Row::Task(t.id)).collect();
    if !state.phases.is_empty() {
        rows.push(Row::NoPhase);
    }
    rows.extend(loose);
    rows.extend(
        runtime
            .scopes
            .scopes
            .iter()
            .map(|s| Row::Scope(s.scope.clone())),
    );
    rows
}

fn row_text(row: &Row, runtime: &Runtime) -> String {
    let state = &runtime.state;
    match row {
        Row::Phase(id) => state.phase(*id).map_or_else(String::new, |p| {
            let goal = p.goal.as_ref().map(|g| format!(" · Goal: {}", sanitize(g)));
            let title = style::bold(&format!("P{} {}", p.id, sanitize(&p.title)));
            format!(
                "{title} [{}]{}",
                p.status.as_str(),
                goal.unwrap_or_default()
            )
        }),
        Row::NoPhase => style::bold("No phase"),
        Row::Task(id) => state
            .task(*id)
            .map_or_else(String::new, |t| format!("  {}", task_line(t, true))),
        Row::Scope(id) => runtime.scopes.get(id).map_or_else(String::new, |s| {
            let (done, total) = TodoState::counts(s.state.tasks.iter());
            let visibility = runtime
                .scopes
                .visibility(id, runtime.config.default_visibility);
            format!(
                "{} {} · {done}/{total} · {} · visibility: {}",
                style::fg(Color::Magenta, s.status.glyph()),
                sanitize(&s.label),
                s.status.as_str(),
                visibility.as_str()
            )
        }),
    }
}

fn scope_tasks(id: &str, runtime: &Runtime) -> Vec<String> {
    let Some(scope) = runtime.scopes.get(id) else {
        return Vec::new();
    };
    if runtime
        .scopes
        .visibility(id, runtime.config.default_visibility)
        != Visibility::All
    {
        return Vec::new();
    }
    let tasks = scope.state.visible_tasks();
    tasks
        .map(|t| format!("      {}", task_line(t, false)))
        .collect()
}

pub fn render(panel: &PanelState, runtime: &Runtime, width: usize) -> Vec<String> {
    let rows = rows(runtime);
    let mut lines = vec![
        format!(
            "{} {}",
            style::bold("Todos"),
            style::dim(&format!("· {HELP}"))
        ),
        style::dim("── Main ──"),
    ];
    if rows.iter().all(|r| matches!(r, Row::Scope(_))) {
        lines.push(style::dim(
            "  No todos yet. Press a to add one, p to add a phase.",
        ));
    }
    let mut scopes_started = false;
    for (index, row) in rows.iter().enumerate() {
        if matches!(row, Row::Scope(_)) && !scopes_started {
            scopes_started = true;
            lines.push(style::dim("── Subagent scopes ──"));
        }
        let text = row_text(row, runtime);
        let line = if index == panel.cursor {
            format!("{} {}", style::fg(Color::Cyan, "▸"), text)
        } else {
            format!("  {text}")
        };
        lines.push(line);
        if let Row::Scope(id) = row {
            lines.extend(scope_tasks(id, runtime));
        }
    }
    lines.push(style::dim("──"));
    match &panel.mode {
        Mode::Normal => lines.push(panel.message.clone()),
        Mode::Input { purpose, buffer } => {
            lines.push(format!("{}: {buffer}█", prompt(purpose)));
            lines.push(style::dim("enter saves · esc cancels"));
        }
        Mode::ConfirmDelete(_) => lines.push("Delete the selected item? y/n".into()),
    }
    lines.iter().map(|l| style::truncate(l, width)).collect()
}

pub fn prompt(purpose: &Purpose) -> String {
    match purpose {
        Purpose::AddTask(None) => "New todo".into(),
        Purpose::AddTask(Some(id)) => format!("New todo in P{id}"),
        Purpose::AddPhase => "New phase".into(),
        Purpose::EditTask(id) => format!("Subject of #{id}"),
        Purpose::EditPhase(id) => format!("Title of P{id}"),
        Purpose::Goal(id) => format!("Goal of P{id} (empty clears)"),
    }
}
