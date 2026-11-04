//! Run state: what each step did. The runner of a run owns its `run.json`; pi sessions read it
//! for the UI, the widget, and reports.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::child;
use crate::launch::Catalog;
use crate::model::{Workflow, format_duration};
use crate::store::write_atomic;

/// A runner writes its heartbeat each second; an active run without one for this long is dead.
pub const STALE_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    #[default]
    Running,
    Waiting,
    Done,
    Failed,
    Cancelled,
    /// pi quit while a foreground run was active, or its runner stopped.
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
    #[serde(skip_serializing_if = "String::is_empty")]
    pub activity: String,
    /// The end of the message the agent writes now.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub writing: String,
}

/// A task that waits for the user's approval.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Approval {
    pub task: String,
    pub step: usize,
    pub asked_ms: u64,
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
    /// Background runs keep going when their pi quits; foreground runs stop with it.
    pub background: bool,
    /// The process id of the pi the run belongs to. That pi reports the end of the run.
    pub owner: u32,
    pub heartbeat_ms: u64,
    pub approval: Option<Approval>,
    /// The pi binary the runner starts agents with.
    pub pi: String,
    pub catalog: Catalog,
    /// What the planner of a dynamic workflow may use.
    #[serde(skip_serializing_if = "Value::is_null")]
    pub available: Value,
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

    /// The step whose agent runs now.
    pub fn agent_step(&self) -> Option<usize> {
        self.steps.iter().position(|s| !s.activity.is_empty())
    }

    pub fn wait_left(&self) -> Option<Duration> {
        let until = self.wait_until_ms?;
        Some(Duration::from_millis(until.saturating_sub(now_ms())))
    }

    /// Active, but the runner stopped writing its heartbeat.
    pub fn stale(&self) -> bool {
        self.status.active() && now_ms().saturating_sub(self.heartbeat_ms) > STALE_MS
    }

    /// Ends the run with `status`. An interrupted run keeps its wait deadline, so it keeps the
    /// rest of the wait when it resumes.
    pub fn end(&mut self, status: RunStatus) {
        self.status = status;
        self.ended_ms = Some(now_ms());
        self.approval = None;
        self.note.clear();
        if status != RunStatus::Interrupted {
            self.wait_until_ms = None;
        }
        for step in &mut self.steps {
            step.activity.clear();
            step.writing.clear();
            if step.status == StepStatus::Running {
                step.status = StepStatus::Pending;
            }
        }
    }

    pub fn interrupt(&mut self) {
        self.end(RunStatus::Interrupted);
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

pub const WRITING_TAIL: usize = 4000;
/// The newest output entries that the run view reads.
const OUTPUT_ENTRIES: usize = 400;

/// The output of all agent runs of step `index`, one JSON entry per line.
pub fn output_file(run_dir: &Path, index: usize) -> PathBuf {
    run_dir
        .join("output")
        .join(format!("{:02}.jsonl", index + 1))
}

/// Appends `entries`; returns how many it wrote.
pub fn append_output(path: &Path, entries: &[child::Entry]) -> usize {
    if entries.is_empty() {
        return 0;
    }
    let mut text = String::new();
    for entry in entries {
        if let Ok(line) = serde_json::to_string(entry) {
            text.push_str(&line);
            text.push('\n');
        }
    }
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?
                .write_all(text.as_bytes())
        });
    if let Err(err) = written {
        pi_ext_sdk::warn!("cannot write {}: {err}", path.display());
    }
    entries.len()
}

pub fn read_output(path: &Path) -> Vec<child::Entry> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let entries: Vec<child::Entry> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let skip = entries.len().saturating_sub(OUTPUT_ENTRIES);
    entries.into_iter().skip(skip).collect()
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

struct Entry {
    state: RunState,
    dir: PathBuf,
    /// When `run.json` changed last, as read by `refresh`.
    modified: Option<SystemTime>,
}

/// Runs by id: the one run of a runner, or the runs on disk that a pi session shows.
#[derive(Clone, Default)]
pub struct Runs {
    inner: Arc<Mutex<BTreeMap<String, Entry>>>,
    /// Keeps the writes of one process in order, so a later write never holds older state.
    writing: Arc<Mutex<()>>,
}

impl Runs {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Entry>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn insert(&self, state: RunState, dir: PathBuf) {
        let id = state.id.clone();
        let entry = Entry {
            state,
            dir,
            modified: None,
        };
        self.lock().insert(id, entry);
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

    /// Writes `run.json` of the run.
    pub fn persist(&self, id: &str) {
        let _writing = self.writing.lock().unwrap_or_else(|e| e.into_inner());
        let Some((state, dir)) = self
            .lock()
            .get(id)
            .map(|e| (e.state.clone(), e.dir.clone()))
        else {
            return;
        };
        save(&dir, &state);
    }

    /// Reads the newest `keep` runs and the active runs it knows from `root`; a `run.json` is
    /// parsed again only when it changed.
    pub fn refresh(&self, root: &Path, keep: usize) {
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        let newest = dirs.len().saturating_sub(keep);
        for (index, dir) in dirs.into_iter().enumerate() {
            let id = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let known = self
                .lock()
                .get(&id)
                .map(|e| (e.modified, e.state.status.active()));
            if index < newest && !known.is_some_and(|(_, active)| active) {
                continue;
            }
            let file = dir.join(RUN_FILE);
            let modified = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
            if modified.is_none() || known.is_some_and(|(seen, _)| seen == modified) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let Ok(state) = serde_json::from_str::<RunState>(&text) else {
                continue;
            };
            self.lock().insert(
                state.id.clone(),
                Entry {
                    state,
                    dir,
                    modified,
                },
            );
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
    fn refresh_reads_changed_runs_and_stale_runs_interrupt() {
        let a = new_id();
        std::thread::sleep(Duration::from_millis(2));
        let b = new_id();
        assert!(a < b && a.starts_with("wf-"));
        let root = std::env::temp_dir().join(format!("wf-runs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut state = RunState {
            id: a.clone(),
            status: RunStatus::Waiting,
            wait_until_ms: Some(5),
            heartbeat_ms: now_ms(),
            steps: vec![StepRun {
                status: StepStatus::Running,
                activity: "bash make".into(),
                ..StepRun::default()
            }],
            ..RunState::default()
        };
        save(&root.join(&a), &state);
        save(
            &root.join(&b),
            &RunState {
                id: b.clone(),
                ..RunState::default()
            },
        );
        let runs = Runs::default();
        runs.refresh(&root, 1);
        assert!(
            runs.get(&b).is_some() && runs.get(&a).is_none(),
            "keeps the newest"
        );
        runs.refresh(&root, 10);
        let loaded = runs.get(&a).unwrap();
        assert_eq!(loaded.steps[0].activity, "bash make");
        assert!(!loaded.stale());
        state.heartbeat_ms = 1;
        assert!(state.stale());
        state.interrupt();
        assert_eq!(state.status, RunStatus::Interrupted);
        assert_eq!(state.wait_until_ms, Some(5));
        assert_eq!(state.steps[0].status, StepStatus::Pending);
        assert!(state.steps[0].activity.is_empty());
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
