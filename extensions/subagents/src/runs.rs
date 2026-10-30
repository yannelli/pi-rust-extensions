//! Running and recent child runs, shared by the tool, the widget, and `/agents runs`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pi_ext_sdk::CancellationToken;
use serde::Serialize;

use crate::stream::Progress;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Running => "running",
            Status::Completed => "completed",
            Status::Failed => "failed",
            Status::Cancelled => "cancelled",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Status::Running => "\u{25d0}",
            Status::Completed => "\u{2713}",
            Status::Failed => "\u{2717}",
            Status::Cancelled => "\u{2298}",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Run {
    pub id: String,
    pub agent: String,
    pub task: String,
    pub background: bool,
    pub status: Status,
    pub started: Instant,
    pub finished: Option<Instant>,
    pub progress: Progress,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub model: Option<String>,
    pub cancel: CancellationToken,
}

impl Run {
    pub fn elapsed(&self) -> Duration {
        self.finished.unwrap_or_else(Instant::now) - self.started
    }

    pub fn label(&self) -> String {
        format!("{}: {}", self.agent, first_line(&self.task, 60))
    }

    /// The text the parent gets: the child's answer, or the failure reason.
    pub fn result_text(&self) -> String {
        let output = self.progress.output();
        match self.status {
            Status::Completed if output.is_empty() => "(no output)".into(),
            Status::Completed => output,
            Status::Cancelled => join_nonempty("Cancelled.", &output),
            Status::Failed => {
                let reason = self.progress.error.clone().unwrap_or_else(|| {
                    let tail = tail_lines(&self.stderr, 8);
                    match self.exit_code {
                        Some(code) => format!("pi exited with code {code}. {tail}"),
                        None => format!("pi stopped. {tail}"),
                    }
                });
                join_nonempty(&format!("Failed: {}", reason.trim()), &output)
            }
            Status::Running => output,
        }
    }
}

#[derive(Clone, Default)]
pub struct Runs {
    inner: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    runs: Vec<Run>,
    next: u64,
}

impl Runs {
    pub fn start(
        &self,
        agent: &str,
        task: &str,
        background: bool,
        cancel: CancellationToken,
    ) -> String {
        let mut state = self.inner.lock().expect("runs lock");
        state.next += 1;
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let id = format!("sa{:x}{}", millis % 0xff_ffff, state.next);
        state.runs.push(Run {
            id: id.clone(),
            agent: agent.to_owned(),
            task: task.to_owned(),
            background,
            status: Status::Running,
            started: Instant::now(),
            finished: None,
            progress: Progress::default(),
            stderr: String::new(),
            exit_code: None,
            model: None,
            cancel,
        });
        id
    }

    pub fn update<R>(&self, id: &str, change: impl FnOnce(&mut Run) -> R) -> Option<R> {
        let mut state = self.inner.lock().expect("runs lock");
        state.runs.iter_mut().find(|run| run.id == id).map(change)
    }

    pub fn get(&self, id: &str) -> Option<Run> {
        self.update(id, |run| run.clone())
    }

    pub fn all(&self) -> Vec<Run> {
        self.inner.lock().expect("runs lock").runs.clone()
    }

    pub fn finish(&self, id: &str, status: Status, keep: usize) {
        let mut state = self.inner.lock().expect("runs lock");
        if let Some(run) = state.runs.iter_mut().find(|run| run.id == id) {
            run.status = status;
            run.finished = Some(Instant::now());
        }
        let finished = state
            .runs
            .iter()
            .filter(|r| r.status != Status::Running)
            .count();
        let mut excess = finished.saturating_sub(keep);
        state.runs.retain(|run| {
            let drop = excess > 0 && run.status != Status::Running;
            excess -= usize::from(drop);
            !drop
        });
    }

    /// Cancels a running run; `false` when it is not running.
    pub fn cancel(&self, id: &str) -> bool {
        self.update(id, |run| {
            let running = run.status == Status::Running;
            run.cancel.cancel();
            running
        })
        .unwrap_or(false)
    }

    pub fn cancel_all(&self) {
        for run in self.inner.lock().expect("runs lock").runs.iter() {
            run.cancel.cancel();
        }
    }
}

pub fn first_line(text: &str, max: usize) -> String {
    let line = text.trim().lines().next().unwrap_or("");
    if line.chars().count() > max {
        let cut: String = line.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}\u{2026}")
    } else {
        line.to_owned()
    }
}

pub fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.trim().lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn join_nonempty(head: &str, body: &str) -> String {
    if body.trim().is_empty() {
        head.to_owned()
    } else {
        format!("{head}\n\n{body}")
    }
}

pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}
