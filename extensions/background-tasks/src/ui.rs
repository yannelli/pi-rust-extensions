use std::sync::Arc;
use std::time::Duration;

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::style::{self, Color};

use crate::registry::{Registry, WidgetMode};
use crate::task::{self, Status, Task};

pub const WIDGET: &str = "background-tasks";
pub const NOTIFICATION: &str = "background-task-notification";
const WIDGET_ROWS: usize = 3;
const EXPANDED_ROWS: usize = 20;
/// More running tasks than this collapse the widget to one line.
pub const COLLAPSE_AT: usize = 3;
const NOTICE_TAIL_BYTES: u64 = 2048;
const NOTICE_TAIL_LINES: usize = 20;

/// The footer status: running and finished counts, `None` without tasks.
pub fn status_text(tasks: &[Task]) -> Option<String> {
    if tasks.is_empty() {
        return None;
    }
    let count = |s: Status| tasks.iter().filter(|t| t.status == s).count();
    let mut parts = vec![format!("{} running", count(Status::Running))];
    for status in [
        Status::Completed,
        Status::Failed,
        Status::Killed,
        Status::TimedOut,
        Status::Lost,
    ] {
        let n = count(status);
        if n > 0 {
            parts.push(format!("{n} {}", status.label()));
        }
    }
    Some(format!("bg: {}", parts.join(", ")))
}

pub fn glyph(status: Status) -> String {
    match status {
        Status::Running => style::fg(Color::Yellow, "●"),
        Status::Completed => style::fg(Color::Green, "✓"),
        Status::Failed | Status::Lost => style::fg(Color::Red, "✗"),
        Status::Killed | Status::TimedOut => style::fg(Color::Magenta, "■"),
    }
}

pub fn task_row(task: &Task, now: u64) -> String {
    format!(
        "{} {} {} {}",
        glyph(task.status),
        task.name,
        style::dim(&format!("({})", task.id)),
        style::dim(&task::format_duration(task.elapsed_ms(now))),
    )
}

/// Widget lines for running tasks; empty when none run. Above `COLLAPSE_AT` running tasks, or
/// when collapsed, one count line points to `/tasks`.
pub fn widget_lines(
    tasks: &[Task],
    now: u64,
    width: Option<usize>,
    mode: WidgetMode,
) -> Vec<String> {
    let running: Vec<&Task> = tasks.iter().filter(|t| t.status.is_running()).collect();
    let collapse = match mode {
        WidgetMode::Auto => running.len() > COLLAPSE_AT,
        WidgetMode::Collapsed => !running.is_empty(),
        WidgetMode::Expanded => false,
    };
    let rows = if mode == WidgetMode::Expanded {
        EXPANDED_ROWS
    } else {
        WIDGET_ROWS
    };
    let mut lines: Vec<String> = if collapse {
        let noun = if running.len() == 1 { "task" } else { "tasks" };
        let count = format!("{} background {noun} running", running.len());
        let hint = style::dim(" · /tasks");
        vec![format!("{} {count}{hint}", glyph(Status::Running))]
    } else {
        running
            .iter()
            .take(rows)
            .map(|t| task_row(t, now))
            .collect()
    };
    if !collapse && running.len() > rows {
        lines.push(style::dim(&format!(
            "+{} more, /tasks",
            running.len() - rows
        )));
    }
    match width {
        Some(width) => lines.iter().map(|l| style::truncate(l, width)).collect(),
        None => lines,
    }
}

/// `/tasks widget [collapse|expand|auto]`: the new mode; no argument toggles.
pub fn widget_mode_for(
    current: WidgetMode,
    running: usize,
    argument: &str,
) -> Result<WidgetMode, String> {
    match argument.trim() {
        "" | "toggle" => {
            let collapsed = match current {
                WidgetMode::Auto => running > COLLAPSE_AT,
                mode => mode == WidgetMode::Collapsed,
            };
            Ok(if collapsed {
                WidgetMode::Expanded
            } else {
                WidgetMode::Collapsed
            })
        }
        "collapse" => Ok(WidgetMode::Collapsed),
        "expand" => Ok(WidgetMode::Expanded),
        "auto" => Ok(WidgetMode::Auto),
        other => Err(format!(
            "usage: /tasks widget [collapse|expand|auto], not {other:?}"
        )),
    }
}

pub fn summary_line(task: &Task) -> String {
    let now = task::now_ms();
    let mut line = format!(
        "{} {} [{}] {} — {}",
        task.id,
        task.name,
        task.status.label(),
        task::format_duration(task.elapsed_ms(now)),
        task.command
    );
    if let Some(code) = task.exit_code {
        line.push_str(&format!(" (exit {code})"));
    }
    line
}

pub fn list_text(tasks: &[Task]) -> String {
    if tasks.is_empty() {
        return "No background tasks.".into();
    }
    tasks
        .iter()
        .map(summary_line)
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn notification_text(task: &Task) -> String {
    let mut lines = vec![
        "<background-task-notification>".to_owned(),
        format!("[bg {}] {} ({})", task.status.label(), task.name, task.id),
        format!("Command: {}", task.command),
    ];
    if let Some(code) = task.exit_code {
        lines.push(format!("Exit code: {code}"));
    }
    if let Some(error) = &task.error {
        lines.push(format!("Error: {error}"));
    }
    lines.push(format!(
        "Duration: {}",
        task::format_duration(task.elapsed_ms(task::now_ms()))
    ));
    let output = task.output_path();
    let slice = task::read_bounded(&output, NOTICE_TAIL_BYTES, true).ok();
    let size = slice.as_ref().map_or(0, |s| s.total);
    lines.push(format!(
        "Output: {} ({})",
        output.display(),
        task::format_size(size)
    ));
    if let Some(slice) = slice.filter(|s| !s.text.trim().is_empty()) {
        let tail: Vec<&str> = slice.text.lines().collect();
        let start = tail.len().saturating_sub(NOTICE_TAIL_LINES);
        lines.push("Last output:".into());
        lines.extend(tail[start..].iter().map(|l| (*l).to_owned()));
    }
    lines.push("</background-task-notification>".into());
    lines.join("\n")
}

/// Redraws the footer status and the widget.
pub async fn refresh(host: &Host, registry: &Registry) -> Result<()> {
    if !host.has_ui() {
        return Ok(());
    }
    let tasks = registry.tasks();
    host.set_status(WIDGET, status_text(&tasks).as_deref())
        .await?;
    let width = host.surface_width(&format!("widget:{WIDGET}"));
    let lines = widget_lines(&tasks, task::now_ms(), width, registry.widget_mode());
    if lines.is_empty() {
        host.clear_widget(WIDGET).await
    } else {
        host.set_widget(WIDGET, lines, Placement::BelowEditor).await
    }
}

pub async fn notify_finished(host: &Host, registry: &Registry, task: &Task) -> Result<()> {
    let message = CustomMessage::new(NOTIFICATION, notification_text(task))
        .display(true)
        .details(task)
        .deliver_as(DeliverAs::FollowUp)
        .trigger_turn(task.trigger_on_completion);
    host.send_message(message).await?;
    registry.mark_notified(&task.id);
    Ok(())
}

/// Polls tasks every 500 ms, sends completion notifications and keeps the UI current.
pub fn spawn_monitor(host: Host, registry: Arc<Registry>) {
    if !registry.monitor_once() {
        return;
    }
    tokio::spawn(async move {
        let mut tick: u64 = 0;
        let mut last_state = String::new();
        loop {
            let finished = registry.poll(tick % 4 == 0).await;
            for task in finished {
                if let Err(err) = notify_finished(&host, &registry, &task).await {
                    pi_ext_sdk::warn!("cannot notify about task {}: {err}", task.id);
                }
            }
            let tasks = registry.tasks();
            let state = format!("{:?}", status_text(&tasks));
            let running = tasks.iter().any(|t| t.status.is_running());
            if state != last_state || (running && tick % 2 == 0) {
                last_state = state;
                if let Err(err) = refresh(&host, &registry).await {
                    pi_ext_sdk::debug!("cannot refresh the task widget: {err}");
                }
            }
            tick += 1;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });
}
