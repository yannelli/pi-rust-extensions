//! Lines of the `rpiv-todos` widget above the editor.

use std::collections::HashSet;

use pi_ext_sdk::style::{self, Color};

use crate::config::{COLLAPSE_KEY_OFF, Config};
use crate::format::sanitize;
use crate::model::{Task, TaskStatus, TodoState};
use crate::scopes::{Scope, Scopes, Visibility};

pub const WIDGET_KEY: &str = "rpiv-todos";
const SCOPE_TASK_BUDGET: usize = 4;

pub struct View<'a> {
    pub state: &'a TodoState,
    pub hidden: &'a HashSet<u64>,
    pub collapsed: bool,
    pub scopes: &'a Scopes,
    pub hidden_scopes: &'a HashSet<String>,
    pub config: &'a Config,
}

/// Tasks of the current phase plus tasks without a phase, minus tombstones and hidden ones.
pub fn main_tasks<'a>(state: &'a TodoState, hidden: &HashSet<u64>) -> Vec<&'a Task> {
    let phase = state.current_phase().map(|p| p.id);
    state
        .visible_tasks()
        .filter(|t| state.phases.is_empty() || t.phase.is_none() || t.phase == phase)
        .filter(|t| !(t.status == TaskStatus::Completed && hidden.contains(&t.id)))
        .collect()
}

pub struct Layout<'a> {
    pub visible: Vec<&'a Task>,
    pub hidden_completed: usize,
    pub truncated_tail: usize,
}

/// rpiv-todo overflow rule: drop completed tasks first, then cut the tail.
pub fn layout<'a>(tasks: &[&'a Task], budget: usize) -> Layout<'a> {
    if tasks.len() <= budget {
        return Layout {
            visible: tasks.to_vec(),
            hidden_completed: 0,
            truncated_tail: 0,
        };
    }
    let inner = budget.saturating_sub(1);
    let open: Vec<&Task> = tasks
        .iter()
        .copied()
        .filter(|t| t.status != TaskStatus::Completed)
        .collect();
    let completed = tasks.len() - open.len();
    if open.len() <= inner {
        let mut room = inner - open.len();
        let visible: Vec<&Task> = tasks
            .iter()
            .copied()
            .filter(|t| {
                let keep = t.status != TaskStatus::Completed || room > 0;
                if t.status == TaskStatus::Completed && keep {
                    room -= 1;
                }
                keep
            })
            .collect();
        let shown = visible
            .iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .count();
        return Layout {
            visible,
            hidden_completed: completed - shown,
            truncated_tail: 0,
        };
    }
    let truncated_tail = open.len() - inner;
    Layout {
        visible: open.into_iter().take(inner).collect(),
        hidden_completed: completed,
        truncated_tail,
    }
}

fn glyph(status: TaskStatus) -> String {
    let color = match status {
        TaskStatus::Pending => return style::dim(status.glyph()),
        TaskStatus::InProgress => Color::Yellow,
        TaskStatus::Completed => Color::Green,
        TaskStatus::Deleted => Color::Red,
    };
    style::fg(color, status.glyph())
}

pub fn task_line(t: &Task, show_id: bool) -> String {
    let subject = sanitize(&t.subject);
    let mut line = glyph(t.status);
    if show_id {
        line += &format!(" {}", style::dim(&format!("#{}", t.id)));
    }
    line += &match t.status {
        TaskStatus::InProgress => format!(" {}", style::fg(Color::Cyan, &subject)),
        TaskStatus::Completed | TaskStatus::Deleted => format!(" {}", style::dim(&subject)),
        TaskStatus::Pending => format!(" {subject}"),
    };
    if let (TaskStatus::InProgress, Some(form)) = (t.status, &t.active_form) {
        line += &format!(" {}", style::dim(&format!("({})", sanitize(form))));
    }
    if !t.blocked_by.is_empty() {
        let deps: Vec<String> = t.blocked_by.iter().map(|id| format!("#{id}")).collect();
        line += &format!(" {}", style::dim(&format!("⛓ {}", deps.join(","))));
    }
    line
}

fn tree_rows(rows: &mut Vec<String>, layout: &Layout, indent: &str, show_ids: bool) {
    for t in &layout.visible {
        rows.push(format!(
            "{indent}{} {}",
            style::dim("├─"),
            task_line(t, show_ids)
        ));
    }
    let hidden = layout.hidden_completed + layout.truncated_tail;
    if hidden > 0 {
        let mut parts = Vec::new();
        if layout.hidden_completed > 0 {
            parts.push(format!("{} completed", layout.hidden_completed));
        }
        if layout.truncated_tail > 0 {
            parts.push(format!("{} pending", layout.truncated_tail));
        }
        let summary = format!("+{hidden} more ({})", parts.join(", "));
        rows.push(format!("{indent}{}", style::dim(&format!("├─ {summary}"))));
    }
}

/// Turns the last `├─` of a tree block into `└─`.
fn close_tree(rows: &mut [String]) {
    if let Some(last) = rows.last_mut() {
        *last = last.replacen("├─", "└─", 1);
    }
}

fn main_section(view: &View) -> Vec<String> {
    let tasks = main_tasks(view.state, view.hidden);
    if tasks.is_empty() {
        return Vec::new();
    }
    let (done, total) = TodoState::counts(tasks.iter().copied());
    let active = tasks
        .iter()
        .any(|t| matches!(t.status, TaskStatus::Pending | TaskStatus::InProgress));
    let paint = |s: &str| {
        if active {
            style::fg(Color::Cyan, s)
        } else {
            style::dim(s)
        }
    };
    let mut heading = format!(
        "{} {}",
        paint(if active { "●" } else { "○" }),
        paint(&format!("Todos ({done}/{total})"))
    );
    let phase = view.state.current_phase();
    if let Some(p) = phase {
        let index = view
            .state
            .phases
            .iter()
            .position(|x| x.id == p.id)
            .unwrap_or(0)
            + 1;
        let count = view.state.phases.len();
        heading += &style::dim(&format!(" · Phase {index}/{count}: {}", sanitize(&p.title)));
    }
    let mut rows = vec![heading];
    if view.collapsed {
        let key = &view.config.collapse_key;
        let hint = if key == COLLAPSE_KEY_OFF {
            "collapsed".to_owned()
        } else {
            format!("{key} to expand")
        };
        rows.push(style::dim(&format!("└─ {hint}")));
        return rows;
    }
    let goal = phase.and_then(|p| p.goal.as_deref());
    if let Some(goal) = goal {
        rows.push(format!(
            "{} {}",
            style::dim("├─"),
            style::italic(&format!("Goal: {}", sanitize(goal)))
        ));
    }
    let budget = view
        .config
        .max_widget_lines
        .saturating_sub(1 + usize::from(goal.is_some()))
        .max(1);
    let show_ids = tasks.iter().any(|t| !t.blocked_by.is_empty());
    tree_rows(&mut rows, &layout(&tasks, budget), "", show_ids);
    close_tree(&mut rows);
    rows
}

fn scope_section(scope: &Scope, visibility: Visibility, rows: &mut Vec<String>) {
    let (done, total) = TodoState::counts(scope.state.tasks.iter());
    let status = scope.status;
    let summary = format!(
        "{} · {done}/{total} · {}",
        sanitize(&scope.label),
        status.as_str()
    );
    rows.push(format!(
        "{} {}",
        style::fg(Color::Magenta, status.glyph()),
        summary
    ));
    if visibility != Visibility::All {
        return;
    }
    let tasks: Vec<&Task> = scope.state.visible_tasks().collect();
    let start = rows.len();
    tree_rows(rows, &layout(&tasks, SCOPE_TASK_BUDGET), "  ", false);
    close_tree(&mut rows[start..]);
}

pub fn render(view: &View, width: usize) -> Vec<String> {
    let mut rows = main_section(view);
    for scope in &view.scopes.scopes {
        let visibility = view
            .scopes
            .visibility(&scope.scope, view.config.default_visibility);
        if visibility == Visibility::MainOnly || view.hidden_scopes.contains(&scope.scope) {
            continue;
        }
        let shown = if view.collapsed {
            Visibility::Collapsed
        } else {
            visibility
        };
        scope_section(scope, shown, &mut rows);
    }
    if rows.is_empty() {
        return rows;
    }
    let mut lines: Vec<String> = rows.iter().map(|r| style::truncate(r, width)).collect();
    lines.push(String::new());
    lines
}
