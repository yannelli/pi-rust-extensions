use pi_ext_sdk::prelude::*;
use pi_ext_sdk::style::{self, Color};

use crate::app::{App, Inner};
use crate::format::{footer_status, truncate_text};
use crate::state::{Goal, GoalStatus, TaskStatus};

pub const WIDGET: &str = "goal";
const COMPACT_TASKS: usize = 5;

fn status_color(goal: &Goal) -> Color {
    match goal.status {
        GoalStatus::Active | GoalStatus::Complete => Color::Green,
        GoalStatus::Auditing => Color::Cyan,
        GoalStatus::Paused => Color::Yellow,
        GoalStatus::Blocked | GoalStatus::BudgetLimited => Color::Red,
    }
}

/// Widget lines: header, task counts, the task tree (compact or expanded), last audit.
pub fn lines(goal: &Goal, expanded: bool, width: usize) -> Vec<String> {
    let mut out = vec![style::fg(status_color(goal), &footer_status(goal))];
    let counts = goal.counts();
    if counts.total > 0 {
        let current = goal
            .current_task_id
            .as_deref()
            .and_then(|id| goal.task(id))
            .map(|task| format!(" · now: {}", task.title))
            .unwrap_or_default();
        out.push(style::dim(&format!(
            "  tasks {}/{} done{current}",
            counts.complete + counts.skipped,
            counts.total
        )));
        let ordered = goal.ordered_tasks();
        let shown: Vec<_> = if expanded {
            ordered
        } else {
            ordered
                .into_iter()
                .filter(|(_, task)| !task.status.is_resolved())
                .take(COMPACT_TASKS)
                .collect()
        };
        for (depth, task) in &shown {
            let line = format!(
                "  {}{} {}",
                "  ".repeat(*depth),
                task.status.glyph(),
                task.title
            );
            out.push(match task.status {
                TaskStatus::InProgress => style::bold(&line),
                TaskStatus::Complete | TaskStatus::Skipped => style::dim(&line),
                TaskStatus::Pending => line,
            });
        }
        let open = counts.pending;
        if !expanded && open > shown.len() {
            out.push(style::dim(&format!(
                "  +{} more · ctrl+shift+t expands",
                open - shown.len()
            )));
        }
    }
    if expanded {
        if let Some(audit) = &goal.last_audit {
            out.push(style::dim(&format!(
                "  last audit: {}",
                truncate_text(audit, 200)
            )));
        }
    }
    out.into_iter()
        .map(|line| style::truncate(&line, width))
        .collect()
}

enum View {
    Goal(Vec<String>),
    Status(String),
    Clear,
}

fn view(inner: &Inner, width: usize) -> View {
    if inner.drafting() {
        return View::Status("goal: drafting · /goal-cancel stops".into());
    }
    if let Some(goal) = inner.focused() {
        return View::Goal(lines(goal, inner.expanded, width));
    }
    let open = inner.open_goals().count();
    if open > 0 && !inner.settings.hide_unfocused_banner {
        return View::Status(format!("goal: {open} open, none focused · /goal-focus"));
    }
    View::Clear
}

pub async fn refresh(app: &App, host: &Host) -> Result<()> {
    if !host.has_ui() {
        return Ok(());
    }
    let width = host
        .surface_width(&format!("widget:{WIDGET}"))
        .unwrap_or(120);
    let view = view(&app.lock(), width);
    match view {
        View::Goal(lines) => {
            host.set_status(WIDGET, None).await?;
            host.set_widget(WIDGET, lines, Placement::AboveEditor).await
        }
        View::Status(text) => {
            host.clear_widget(WIDGET).await?;
            host.set_status(WIDGET, Some(&text)).await
        }
        View::Clear => {
            host.clear_widget(WIDGET).await?;
            host.set_status(WIDGET, None).await
        }
    }
}
