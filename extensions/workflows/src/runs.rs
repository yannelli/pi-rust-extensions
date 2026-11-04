//! Run state: what each step did, kept in memory for the UI and in `run.json` for history and
//! resume.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pi_ext_sdk::CancellationToken;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::model::{Workflow, format_duration};
use crate::store::write_atomic;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    #[default]
    Running,
    Waiting,
    Done,
    Failed,
    Cancelled,
    /// pi quit while the run was active.
    Interrupted,
}

impl RunStatus {
    pub fn active(self) -> bool {
        matches!(self, RunStatus::Running | RunStatus::Waiting)
    }

    pub fn label(self) -> &'static str {
        match self {
            RunStatus::Running => "running",
            RunStatus::Waiting => "waiting",
            RunStatus::Done => "done",
            RunStatus::Failed => "failed",
            RunStatus::Cancelled => "stopped",
            RunStatus::Interrupted => "interrupted",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    #[default]
    Pending,
    Running,
    Done,
    Failed,
}

impl StepStatus {
    pub fn label(self) -> &'static str {
        match self {
            StepStatus::Pending => "pending",
            StepStatus::Running => "running",
            StepStatus::Done => "done",
            StepStatus::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    #[default]
    User,
    Agent,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StepRun {
    pub status: StepStatus,
    /// Tasks finished; resume starts at this task.
    pub tasks_done: usize,
    /// Agent runs of the step, repeats included.
    pub runs: u32,
    pub started_ms: Option<u64>,
    pub ended_ms: Option<u64>,
    /// The last report of the step's agent.
    pub summary: String,
    pub cost: f64,
    pub turns: u32,
    pub model: Option<String>,
    /// One line for each agent run.
    pub log: Vec<String>,
    /// The tool or text the agent works on now.
    #[serde(skip)]
    pub activity: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RunState {
    pub id: String,
    pub workflow: Workflow,
    pub inputs: BTreeMap<String, String>,
    pub cwd: String,
    pub origin: Origin,
    pub status: RunStatus,
    pub started_ms: u64,
    pub ended_ms: Option<u64>,
    /// One entry for each step of `workflow`.
    pub steps: Vec<StepRun>,
    pub wait_until_ms: Option<u64>,
    /// What the run waits for, or the planner's last change.
    pub note: String,
    pub error: Option<String>,
    /// The model and thinking of the session that started the run; steps without a model use them.
    pub parent_model: Option<String>,
    pub parent_thinking: Option<String>,
}

impl RunState {
    pub fn cost(&self) -> f64 {
        self.steps.iter().map(|s| s.cost).sum()
    }

    pub fn elapsed(&self) -> Duration {
        let end = self.ended_ms.unwrap_or_else(now_ms);
        Duration::from_millis(end.saturating_sub(self.started_ms))
    }

    /// The step that runs now, or the first unfinished one.
    pub fn current_step(&self) -> Option<usize> {
        self.steps
            .iter()
            .position(|s| s.status == StepStatus::Running)
            .or_else(|| self.steps.iter().position(|s| s.status != StepStatus::Done))
    }

    pub fn wait_left(&self) -> Option<Duration> {
        let until = self.wait_until_ms?;
        Some(Duration::from_millis(until.saturating_sub(now_ms())))
    }

    /// `12m4s · $1.84`.
    pub fn stats(&self) -> String {
        let mut text = format_duration(Duration::from_secs(self.elapsed().as_secs()));
        let cost = self.cost();
        if cost > 0.0 {
            text.push_str(&format!(" · ${cost:.2}"));
        }
        text
    }
}

impl StepRun {
    pub fn elapsed(&self) -> Option<Duration> {
        let start = self.started_ms?;
        let end = self.ended_ms.unwrap_or_else(now_ms);
        Some(Duration::from_millis(end.saturating_sub(start)))
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// A short id that sorts by start time.
pub fn new_id() -> String {
    let mut n = now_ms();
    let mut out = Vec::new();
    while n > 0 {
        out.push(char::from_digit((n % 36) as u32, 36).unwrap_or('0'));
        n /= 36;
    }
    out.reverse();
    format!("wf-{}", out.into_iter().collect::<String>())
}

/// Stop and skip-the-wait signals of an active run.
#[derive(Clone, Default)]
pub struct Control {
    pub cancel: CancellationToken,
    pub skip: Arc<Notify>,
    /// Set when pi quits: the run stops as interrupted, so it can resume later.
    pub interrupted: Arc<AtomicBool>,
}

struct Entry {
    state: RunState,
    control: Option<Control>,
    dir: PathBuf,
}

/// The runs of this pi process and the ones loaded from disk.
#[derive(Clone, Default)]
pub struct Runs {
    inner: Arc<Mutex<BTreeMap<String, Entry>>>,
}

impl Runs {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Entry>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn insert(&self, state: RunState, dir: PathBuf, control: Option<Control>) {
        let id = state.id.clone();
        self.lock().insert(
            id,
            Entry {
                state,
                control,
                dir,
            },
        );
    }

    pub fn get(&self, id: &str) -> Option<RunState> {
        self.lock().get(id).map(|e| e.state.clone())
    }

    /// Newest first.
    pub fn all(&self) -> Vec<RunState> {
        self.lock()
            .values()
            .rev()
            .map(|e| e.state.clone())
            .collect()
    }

    pub fn any_active(&self) -> bool {
        self.lock().values().any(|e| e.state.status.active())
    }

    pub fn update<R>(&self, id: &str, edit: impl FnOnce(&mut RunState) -> R) -> Option<R> {
        self.lock().get_mut(id).map(|e| edit(&mut e.state))
    }

    pub fn dir(&self, id: &str) -> Option<PathBuf> {
        self.lock().get(id).map(|e| e.dir.clone())
    }

    pub fn set_control(&self, id: &str, control: Option<Control>) {
        if let Some(entry) = self.lock().get_mut(id) {
            entry.control = control;
        }
    }

    pub fn control(&self, id: &str) -> Option<Control> {
        self.lock().get(id).and_then(|e| e.control.clone())
    }

    /// Writes `run.json` of the run.
    pub fn persist(&self, id: &str) {
        let Some((state, dir)) = self
            .lock()
            .get(id)
            .map(|e| (e.state.clone(), e.dir.clone()))
        else {
            return;
        };
        save(&dir, &state);
    }

    /// Loads the newest runs from `root`; active runs of an earlier pi become interrupted.
    pub fn load(&self, root: &Path, keep: usize) {
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for dir in dirs.into_iter().rev().take(keep) {
            let Ok(text) = std::fs::read_to_string(dir.join(RUN_FILE)) else {
                continue;
            };
            let Ok(mut state) = serde_json::from_str::<RunState>(&text) else {
                continue;
            };
            if self.lock().contains_key(&state.id) {
                continue;
            }
            if state.status.active() {
                state.status = RunStatus::Interrupted;
                state.wait_until_ms = None;
                for step in &mut state.steps {
                    if step.status == StepStatus::Running {
                        step.status = StepStatus::Pending;
                    }
                }
                save(&dir, &state);
            }
            self.insert(state, dir, None);
        }
    }
}

pub const RUN_FILE: &str = "run.json";
const RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);

fn save(dir: &Path, state: &RunState) {
    let Ok(text) = serde_json::to_string_pretty(state) else {
        return;
    };
    if let Err(err) = write_atomic(&dir.join(RUN_FILE), &text) {
        pi_ext_sdk::warn!("cannot write {}: {err}", dir.display());
    }
}

/// Deletes run directories older than 30 days.
pub fn prune(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > RETENTION);
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_sort_by_time_and_load_marks_interrupted() {
        let a = new_id();
        std::thread::sleep(Duration::from_millis(2));
        let b = new_id();
        assert!(a < b && a.starts_with("wf-"));
        let root = std::env::temp_dir().join(format!("wf-runs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let state = RunState {
            id: a.clone(),
            status: RunStatus::Waiting,
            wait_until_ms: Some(1),
            steps: vec![StepRun {
                status: StepStatus::Running,
                ..StepRun::default()
            }],
            ..RunState::default()
        };
        save(&root.join(&a), &state);
        let runs = Runs::default();
        runs.load(&root, 10);
        let loaded = runs.get(&a).unwrap();
        assert_eq!(loaded.status, RunStatus::Interrupted);
        assert_eq!(loaded.wait_until_ms, None);
        assert_eq!(loaded.steps[0].status, StepStatus::Pending);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn current_step_and_cost() {
        let state = RunState {
            steps: vec![
                StepRun {
                    status: StepStatus::Done,
                    cost: 0.5,
                    ..StepRun::default()
                },
                StepRun {
                    cost: 0.25,
                    ..StepRun::default()
                },
            ],
            ..RunState::default()
        };
        assert_eq!(state.current_step(), Some(1));
        assert!((state.cost() - 0.75).abs() < 1e-9);
    }
}
