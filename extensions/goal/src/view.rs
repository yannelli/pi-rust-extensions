//! Text views of a goal for `get_goal` and `/goal-status`.

use crate::format::{format_duration, objective_title, status_label, truncate_text};
use crate::state::Goal;

pub const PAGE_CHARS: usize = 4_000;

pub fn summary(goal: &Goal, with_audit: bool) -> String {
    let counts = goal.counts();
    let mut lines = vec![
        format!("Goal {} — {}", goal.id, status_label(goal)),
        format!("Objective: {}", objective_title(&goal.objective)),
        format!(
            "Usage: {} active, {} tokens{}",
            format_duration(goal.usage.active_seconds),
            goal.usage.tokens_used,
            goal.token_budget
                .map(|b| format!(" of {b} budget"))
                .unwrap_or_default()
        ),
        format!("Autonomous runs since start/resume: {}", goal.runs),
    ];
    if counts.total > 0 {
        lines.push(format!(
            "Tasks: {}/{} complete, {} skipped, {} pending{}",
            counts.complete,
            counts.total,
            counts.skipped,
            counts.pending,
            if goal.block_completion {
                " (completion gated)"
            } else {
                ""
            }
        ));
    }
    if let Some(task) = goal.current_task_id.as_deref().and_then(|id| goal.task(id)) {
        lines.push(format!("Current task: {} · {}", task.id, task.title));
    }
    if let Some(next) = goal
        .ordered_tasks()
        .into_iter()
        .find(|(_, t)| !t.status.is_resolved())
    {
        lines.push(format!("Next open task: {} · {}", next.1.id, next.1.title));
    }
    if let Some(wait) = &goal.wait {
        lines.push(format!("Waiting until {}: {}", wait.deadline, wait.reason));
    }
    if with_audit {
        if let Some(audit) = &goal.last_audit {
            lines.push(format!(
                "Last audit ({}): {}",
                goal.audits,
                truncate_text(audit, 600)
            ));
        }
    }
    lines.join("\n")
}

pub fn objective(goal: &Goal) -> String {
    let mut text = format!("Objective (goalId={}):\n{}", goal.id, goal.objective);
    if let Some(contract) = &goal.verification_contract {
        text.push_str(&format!("\n\nVerification contract:\n{contract}"));
    }
    text
}

fn task_line(depth: usize, task: &crate::state::GoalTask) -> String {
    let mut line = format!(
        "{}{} {}: {}",
        "  ".repeat(depth),
        task.status.glyph(),
        task.id,
        task.title
    );
    if task.lightweight_subtasks {
        line.push_str(" (lightweight)");
    }
    if let Some(contract) = &task.verification_contract {
        line.push_str(&format!(" — contract: {contract}"));
    }
    if let Some(evidence) = &task.evidence {
        line.push_str(&format!(" — evidence: {evidence}"));
    }
    if let Some(reason) = &task.skip_reason {
        line.push_str(&format!(" — skipped: {reason}"));
    }
    line
}

pub fn tasks(goal: &Goal, task_id: Option<&str>) -> Result<String, String> {
    if let Some(id) = task_id {
        let task = goal
            .task(id)
            .ok_or_else(|| format!("Unknown task id \"{id}\"."))?;
        return Ok(task_line(0, task));
    }
    if goal.tasks.is_empty() {
        return Ok("No tasks. Use set_goal_tasks to add a task tree.".into());
    }
    let counts = goal.counts();
    let mut lines = vec![format!(
        "Tasks {}/{} complete:",
        counts.complete, counts.total
    )];
    lines.extend(
        goal.ordered_tasks()
            .into_iter()
            .map(|(d, t)| task_line(d, t)),
    );
    Ok(lines.join("\n"))
}

pub fn history(goal: &Goal) -> String {
    let mut lines = vec![format!("History ({} events):", goal.history.len())];
    lines.extend(
        goal.history
            .iter()
            .map(|h| format!("- {} {}", h.at, h.event)),
    );
    lines.join("\n")
}

pub fn verbose(goal: &Goal) -> String {
    let tasks = tasks(goal, None).unwrap_or_default();
    [summary(goal, true), objective(goal), tasks, history(goal)].join("\n\n")
}

/// One `PAGE_CHARS` page of `text` from `cursor` (a character offset).
pub fn page(text: &str, cursor: Option<&str>) -> Result<String, String> {
    let start = match cursor.map(str::trim).filter(|c| !c.is_empty()) {
        Some(cursor) => cursor
            .parse::<usize>()
            .map_err(|_| "cursor must come from a previous page.".to_owned())?,
        None => 0,
    };
    let total = text.chars().count();
    if start > total {
        return Err("cursor is past the end of this section.".into());
    }
    let body: String = text.chars().skip(start).take(PAGE_CHARS).collect();
    let end = start + body.chars().count();
    Ok(if end < total {
        format!("{body}\n\n[more: repeat the call with cursor=\"{end}\"]")
    } else {
        body
    })
}
