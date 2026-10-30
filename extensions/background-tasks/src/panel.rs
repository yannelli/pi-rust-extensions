use std::sync::Arc;
use std::time::Duration;

use pi_ext_sdk::prelude::*;

use crate::registry::Registry;
use crate::task::{self, Task};
use crate::ui;

const LOG_LINES: usize = 15;
const LOG_BYTES: u64 = 8 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct View {
    pub selected: usize,
    pub logs: bool,
    pub armed_stop: bool,
    pub message: Option<String>,
}

pub enum Action {
    None,
    Close,
    Stop(String),
    ClearFinished,
}

impl View {
    /// Applies one key to the view; returns the side effect the caller performs.
    pub fn key(&mut self, key: &Key, tasks: &[Task]) -> Action {
        let armed = std::mem::take(&mut self.armed_stop);
        self.message = None;
        if key.is_cancel() || key.is("left") {
            if self.logs {
                self.logs = false;
                return Action::None;
            }
            return Action::Close;
        }
        let last = tasks.len().saturating_sub(1);
        match key.char() {
            _ if key.is("up") => self.selected = self.selected.saturating_sub(1),
            _ if key.is("down") => self.selected = (self.selected + 1).min(last),
            Some('k') => self.selected = self.selected.saturating_sub(1),
            Some('j') => self.selected = (self.selected + 1).min(last),
            _ if key.is("enter") || key.is("right") => self.logs = !tasks.is_empty(),
            Some('l') => self.logs = !tasks.is_empty(),
            Some('c') if !self.logs => return Action::ClearFinished,
            Some('x') => match tasks.get(self.selected) {
                Some(task) if task.status.is_running() && armed => {
                    return Action::Stop(task.id.clone());
                }
                Some(task) if task.status.is_running() => {
                    self.armed_stop = true;
                    self.message = Some(format!("Press x again to stop {}", task.name));
                }
                Some(_) => self.message = Some("Task already finished".into()),
                None => {}
            },
            _ => {}
        }
        Action::None
    }

    pub fn render(&mut self, tasks: &[Task], width: Option<usize>) -> Vec<String> {
        self.selected = self.selected.min(tasks.len().saturating_sub(1));
        let now = task::now_ms();
        let mut lines = match tasks.get(self.selected).filter(|_| self.logs) {
            Some(task) => log_lines(task),
            None => list_lines(tasks, self.selected, now),
        };
        if let Some(message) = &self.message {
            lines.push(style::bold(message));
        }
        let help = if self.logs {
            "esc back · x stop"
        } else {
            "↑↓ select · enter logs · x stop · c clear finished · esc close"
        };
        lines.push(style::dim(help));
        match width {
            Some(width) => lines.iter().map(|l| style::truncate(l, width)).collect(),
            None => lines,
        }
    }
}

fn list_lines(tasks: &[Task], selected: usize, now: u64) -> Vec<String> {
    let mut lines = vec![style::bold("Background tasks")];
    if tasks.is_empty() {
        lines.push(style::dim(
            "No background tasks. Start one with /bg <command>.",
        ));
    }
    for (index, task) in tasks.iter().enumerate() {
        let marker = if index == selected { "›" } else { " " };
        let row = format!(
            "{marker} {}  {}",
            ui::task_row(task, now),
            style::dim(&task.command)
        );
        lines.push(if index == selected {
            style::inverse(&row)
        } else {
            row
        });
    }
    lines
}

fn log_lines(task: &Task) -> Vec<String> {
    let mut lines = vec![
        format!(
            "{} {} {}",
            ui::glyph(task.status),
            style::bold(&task.name),
            style::dim(&format!("({})", task.id))
        ),
        style::dim(&format!("$ {}", task.command)),
    ];
    if let Some(code) = task.exit_code {
        lines.push(style::dim(&format!("exit {code}")));
    }
    if let Some(error) = &task.error {
        lines.push(style::dim(error));
    }
    match task::read_bounded(&task.output_path(), LOG_BYTES, true) {
        Ok(slice) if slice.text.is_empty() => lines.push(style::dim("(no output yet)")),
        Ok(slice) => {
            let all: Vec<&str> = slice.text.lines().collect();
            let start = all.len().saturating_sub(LOG_LINES);
            lines.extend(all[start..].iter().map(|l| (*l).to_owned()));
        }
        Err(err) => lines.push(format!("cannot read output: {err}")),
    }
    lines.push(style::dim(&format!(
        "Output: {}",
        task.output_path().display()
    )));
    lines
}

/// `/tasks`: a list of tasks with live refresh, log view, stop and clear.
pub async fn open(host: Host, registry: Arc<Registry>, focus: String) -> Result<()> {
    if !host.has_ui() {
        pi_ext_sdk::info!(
            "/tasks needs an interactive UI; use /jobs, /logs, bg_status, or bg_logs"
        );
        return Ok(());
    }
    let mut view = View::default();
    if !focus.is_empty() {
        if let Ok(task) = registry.resolve(&focus) {
            view.selected = registry
                .tasks()
                .iter()
                .position(|t| t.id == task.id)
                .unwrap_or(0);
            view.logs = true;
        }
    }
    let lines = view.render(&registry.tasks(), None);
    let options = json!({ "anchor": "bottom-center", "width": "96%", "minWidth": 64, "maxHeight": "60%", "margin": { "bottom": 1, "left": 1, "right": 1 } });
    let mut panel = host.panel(lines).overlay_options(options).open().await?;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            event = panel.next_event() => match event {
                PanelEvent::Key(key) => match view.key(&key, &registry.tasks()) {
                    Action::Close => {
                        panel.close(Value::Null).await?;
                        return Ok(());
                    }
                    Action::Stop(id) => {
                        view.message = Some(match registry.stop(&id, "stopped from /tasks").await {
                            Ok(task) => format!("Stopped {}", task.name),
                            Err(err) => err,
                        });
                        let _ = ui::refresh(&host, &registry).await;
                    }
                    Action::ClearFinished => {
                        let removed = registry.clear_finished();
                        view.message = Some(format!("Removed {removed} finished task(s)"));
                        let _ = ui::refresh(&host, &registry).await;
                    }
                    Action::None => {}
                },
                PanelEvent::Resize(_) => {}
                PanelEvent::Closed => return Ok(()),
            },
            _ = tick.tick() => {}
        }
        panel.set_lines(view.render(&registry.tasks(), panel.width()))?;
    }
}
