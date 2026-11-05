//! The extension's side of runs: start and resume runners, send them commands, and watch the
//! runs on disk to report their ends, ask for approvals, and draw the widget.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pi_ext_sdk::prelude::*;

use crate::control::{self, Command, Decision};
use crate::launch::{Catalog, REPORT_SCHEMA, pi_binary};
use crate::model::{Mode, Workflow};
use crate::planner;
use crate::runner::{self, LOG_FILE, MEMORY_FILE, SCHEMA_FILE};
use crate::runs::{Origin, RunState, RunStatus, Runs, StepStatus, new_id, now_ms};
use crate::store::{self, Dirs};
use crate::ui::widget;

pub const MESSAGE_TYPE: &str = "workflow-run";
/// How many runs `/workflows` and `status` show besides the active ones.
const KEEP_RUNS: usize = 30;
const WATCH_EVERY: Duration = Duration::from_secs(1);

#[derive(Clone, Default)]
pub struct App {
    pub runs: Runs,
    watch: Arc<Mutex<Watch>>,
}

/// What this pi session already did for the runs it watches.
#[derive(Default)]
struct Watch {
    /// Run ends (`id`, `endedMs`) checked for a report.
    reported: HashSet<(String, u64)>,
    /// Approvals (`id`, `askedMs`) this session asked about.
    asked: HashSet<(String, u64)>,
    announced: bool,
    widget_shown: bool,
    paint: Option<crate::ui::theme::Paint>,
    /// The reason of `session_shutdown`.
    shutdown_reason: Option<String>,
}

pub fn trusted(host: &Host) -> bool {
    host.snapshot().is_some_and(|s| s.is_project_trusted)
}

impl App {
    fn watch(&self) -> std::sync::MutexGuard<'_, Watch> {
        self.watch.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn dirs(&self, host: &Host) -> Dirs {
        Dirs::new(&host.agent_dir(), &host.project_dir(), trusted(host))
    }

    pub fn runs_root(&self, host: &Host) -> PathBuf {
        store::runs_dir(&host.agent_dir())
    }

    /// Reads the runs on disk again.
    pub fn refresh(&self, host: &Host) {
        self.runs.refresh(&self.runs_root(host), KEEP_RUNS);
    }

    /// Skills and MCP servers that steps can name.
    pub async fn catalog(&self, host: &Host) -> Catalog {
        let mut catalog = Catalog::default();
        for command in host.all_commands().await.unwrap_or_default() {
            let name = command["name"]
                .as_str()
                .and_then(|n| n.strip_prefix("skill:"));
            if let (Some(name), Some(path)) = (name, command["sourceInfo"]["path"].as_str()) {
                catalog.skills.insert(name.to_owned(), path.to_owned());
            }
        }
        let mut servers: Vec<String> = Vec::new();
        let mut configs = vec![host.agent_dir().join("mcp.json")];
        if trusted(host) {
            configs.push(host.project_file("mcp.json"));
        }
        for path in configs {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let config: Value = serde_json::from_str(&text).unwrap_or_default();
            if let Some(map) = config["mcpServers"].as_object() {
                servers.extend(map.keys().cloned());
            }
        }
        for tool in host.all_tools().await.unwrap_or_default() {
            let server = tool["name"]
                .as_str()
                .and_then(|n| n.strip_prefix("mcp__"))
                .and_then(|n| n.split_once("__"));
            if let Some((server, _)) = server {
                servers.push(server.to_owned());
            }
        }
        for server in servers {
            let id = crate::launch::mcp_id(&server);
            if !catalog
                .mcp_servers
                .iter()
                .any(|s| crate::launch::mcp_id(s) == id)
            {
                catalog.mcp_servers.push(server);
            }
        }
        catalog
    }

    /// Starts a runner for the workflow and returns the run id.
    pub async fn start(
        &self,
        host: &Host,
        workflow: Workflow,
        inputs: BTreeMap<String, String>,
        origin: Origin,
        background: bool,
    ) -> std::result::Result<String, String> {
        let problems = workflow.problems();
        if !problems.is_empty() {
            return Err(problems.join("\n"));
        }
        let missing: Vec<String> = workflow
            .inputs()
            .into_iter()
            .filter(|name| inputs.get(name).is_none_or(|v| v.trim().is_empty()))
            .collect();
        if !missing.is_empty() {
            return Err(format!("missing inputs: {}", missing.join(", ")));
        }
        let catalog = self.catalog(host).await;
        let mut unknown: Vec<String> = Vec::new();
        for step in &workflow.steps {
            unknown.extend(workflow.scope(step).skills.unwrap_or_default());
            unknown.extend(step.tasks.iter().filter_map(|t| t.skill.clone()));
        }
        unknown.retain(|name| !catalog.skills.contains_key(name));
        unknown.sort();
        unknown.dedup();
        if !unknown.is_empty() {
            return Err(format!("unknown skills: {}", unknown.join(", ")));
        }
        let available = match workflow.mode {
            Mode::Dynamic => planner::available(host, &catalog).await,
            Mode::Fixed => Value::Null,
        };
        let id = new_id();
        let dir = self.runs_root(host).join(&id);
        create_run_dir(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let snapshot = host.snapshot();
        let state = RunState {
            id: id.clone(),
            steps: vec![Default::default(); workflow.steps.len()],
            workflow,
            inputs,
            cwd: host.cwd().display().to_string(),
            origin,
            status: RunStatus::Running,
            started_ms: now_ms(),
            parent_model: snapshot.as_ref().and_then(|s| s.model_ref()),
            parent_thinking: snapshot.as_ref().and_then(|s| s.thinking_level.clone()),
            background,
            owner: control::pi_pid(),
            heartbeat_ms: now_ms(),
            pi: pi_binary().display().to_string(),
            catalog,
            available,
            ..RunState::default()
        };
        self.runs.insert(state, dir.clone());
        self.runs.persist(&id);
        control::touch(&store::root(&host.agent_dir()), control::pi_pid());
        spawn_runner(&dir).map_err(|e| format!("cannot start the runner: {e}"))?;
        Ok(id)
    }

    /// Starts a new runner for a stopped, failed, or interrupted run; it continues at the task
    /// that did not finish.
    pub async fn resume(&self, host: &Host, id: &str) -> std::result::Result<(), String> {
        self.refresh(host);
        let run = self.runs.get(id).ok_or_else(|| format!("no run {id}"))?;
        if run.status.active() && !run.stale() {
            return Err(format!("run {id} is still {}", run.status.label()));
        }
        if run.status == RunStatus::Done {
            return Err(format!("run {id} is done"));
        }
        let dir = self.runs.dir(id).ok_or_else(|| format!("no run {id}"))?;
        let catalog = self.catalog(host).await;
        let available = match run.workflow.mode {
            Mode::Dynamic => planner::available(host, &catalog).await,
            Mode::Fixed => Value::Null,
        };
        self.runs.update(id, |run| {
            if run.status.active() {
                run.interrupt();
            }
            run.status = RunStatus::Running;
            run.error = None;
            run.ended_ms = None;
            run.owner = control::pi_pid();
            run.heartbeat_ms = now_ms();
            run.pi = pi_binary().display().to_string();
            run.catalog = catalog;
            run.available = available;
            for step in &mut run.steps {
                if step.status == StepStatus::Failed {
                    step.status = StepStatus::Pending;
                }
            }
        });
        self.runs.persist(id);
        control::touch(&store::root(&host.agent_dir()), control::pi_pid());
        spawn_runner(&dir).map_err(|e| format!("cannot start the runner: {e}"))
    }

    fn send(&self, id: &str, command: &Command) -> std::result::Result<(), String> {
        let dir = self.runs.dir(id).ok_or_else(|| format!("no run {id}"))?;
        control::send(&dir, command).map_err(|e| e.to_string())
    }

    fn active(&self, id: &str) -> std::result::Result<RunState, String> {
        match self.runs.get(id) {
            Some(run) if run.status.active() => Ok(run),
            Some(_) => Err(format!("Run {id} is not active.")),
            None => Err(format!("No run {id}.")),
        }
    }

    pub fn stop(&self, id: &str) -> std::result::Result<(), String> {
        self.active(id)?;
        self.send(id, &Command::Stop)
    }

    /// Passes `message` to the agent that runs now; it reads it after its current tool call.
    pub fn steer(&self, id: &str, message: &str) -> std::result::Result<(), String> {
        if self.active(id)?.agent_step().is_none() {
            return Err(format!("No agent of run {id} works now."));
        }
        let dir = self.runs.dir(id).ok_or_else(|| format!("no run {id}"))?;
        control::steer(&dir.join(control::INBOX_DIR), message.trim()).map_err(|e| e.to_string())
    }

    /// Ends the current wait of a run at once.
    pub fn skip_wait(&self, id: &str) -> std::result::Result<(), String> {
        if self.active(id)?.wait_until_ms.is_none() {
            return Err(format!("Run {id} is not waiting."));
        }
        self.send(id, &Command::Skip)
    }

    pub fn approve(&self, id: &str, decision: Decision) -> std::result::Result<String, String> {
        let run = self.active(id)?;
        let approval = run
            .approval
            .ok_or_else(|| format!("Run {id} does not wait for an approval."))?;
        self.send(id, &Command::Approve { decision })?;
        let word = match decision {
            Decision::Run => "Approved",
            Decision::Skip => "Skipped",
            Decision::Stop => "Stopping the run at",
        };
        Ok(format!(
            "{word} \"{}\" of {}.",
            approval.task, run.workflow.name
        ))
    }

    /// A background run keeps going after its pi quits; a foreground run follows this pi.
    pub fn set_background(&self, id: &str, background: bool) -> std::result::Result<(), String> {
        self.active(id)?;
        self.send(
            id,
            &Command::Mode {
                background,
                owner: control::pi_pid(),
            },
        )?;
        self.runs.update(id, |run| {
            run.background = background;
            run.owner = control::pi_pid();
        });
        Ok(())
    }

    pub fn on_shutdown_event(&self, reason: Option<String>) {
        self.watch().shutdown_reason = reason;
    }

    /// When pi quits, its foreground runs stop as interrupted. `/reload` and session switches
    /// keep the pi process, so its runs go on.
    pub fn shutdown(&self, host: &Host) {
        let reason = self.watch().shutdown_reason.clone();
        if reason.as_deref().is_some_and(|r| r != "quit") {
            return;
        }
        let me = control::pi_pid();
        for run in self.runs.all() {
            if run.status.active() && !run.background && run.owner == me {
                let _ = self.send(&run.id, &Command::Interrupt);
            }
        }
        control::forget(&store::root(&host.agent_dir()), me);
    }

    /// Keeps this pi's liveness file fresh and handles the runs on disk once a second.
    pub async fn watch_runs(&self, host: Host) {
        let root = store::root(&host.agent_dir());
        let me = control::pi_pid();
        let mut touched = tokio::time::Instant::now() - control::TOUCH_EVERY;
        loop {
            if touched.elapsed() >= control::TOUCH_EVERY {
                control::touch(&root, me);
                touched = tokio::time::Instant::now();
            }
            self.refresh(&host);
            self.tick(&host, &root, me).await;
            tokio::time::sleep(WATCH_EVERY).await;
        }
    }

    async fn tick(&self, host: &Host, root: &Path, me: u32) {
        let runs = self.runs.all();
        for run in runs.iter().filter(|r| r.stale()) {
            self.runs.update(&run.id, RunState::interrupt);
            self.runs.persist(&run.id);
        }
        let ui = host.has_ui();
        for run in &runs {
            let Some(ended) = run.ended_ms.filter(|_| !run.status.active()) else {
                continue;
            };
            let key = (run.id.clone(), ended);
            if self.watch().reported.contains(&key) {
                continue;
            }
            // The owner reports; a session with a UI reports for an owner that is gone.
            let reporter =
                run.owner != 0 && (run.owner == me || ui && !control::alive(root, run.owner));
            if !reporter {
                continue;
            }
            self.watch().reported.insert(key);
            if self.claim(&run.id, ended) {
                self.report(host, run, run.owner == me).await;
            }
        }
        if !ui {
            return;
        }
        for run in &runs {
            if let Some(approval) = run.approval.as_ref().filter(|_| run.status.active())
                && self
                    .watch()
                    .asked
                    .insert((run.id.clone(), approval.asked_ms))
            {
                tokio::spawn(ask_approval(self.clone(), host.clone(), run.clone()));
            }
        }
        let active: Vec<RunState> = runs.into_iter().filter(|r| r.status.active()).collect();
        let elsewhere = active
            .iter()
            .filter(|r| r.owner != me || r.background)
            .count();
        let announce = !std::mem::replace(&mut self.watch().announced, true);
        if announce && elsewhere > 0 {
            let text = match elsewhere {
                1 => "1 workflow run is active in the background. Open /workflows runs to view it."
                    .to_owned(),
                n => format!(
                    "{n} workflow runs are active in the background. Open /workflows runs to view them."
                ),
            };
            let _ = host.notify_info(text).await;
        }
        self.draw_widget(host, &active).await;
    }

    /// The first pi to create the claim file of a run end reports it.
    fn claim(&self, id: &str, ended: u64) -> bool {
        let Some(dir) = self.runs.dir(id) else {
            return false;
        };
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(format!("reported-{ended}")))
            .is_ok()
    }

    async fn report(&self, host: &Host, run: &RunState, own: bool) {
        let message = CustomMessage::new(MESSAGE_TYPE, runner::report_text(run))
            .display(true)
            .details(runner::report_details(run))
            .deliver_as(DeliverAs::FollowUp)
            .trigger_turn(own && run.origin == Origin::Agent);
        if let Err(err) = host.send_message(message).await {
            pi_ext_sdk::warn!("cannot report workflow run {}: {}", run.id, err.message());
        }
    }

    async fn draw_widget(&self, host: &Host, active: &[RunState]) {
        if active.is_empty() {
            if std::mem::take(&mut self.watch().widget_shown) {
                let _ = host.clear_widget(widget::KEY).await;
            }
            return;
        }
        self.watch().widget_shown = true;
        let width = host
            .surface_width(&format!("widget:{}", widget::KEY))
            .unwrap_or(100);
        let cached = self.watch().paint.clone();
        let paint = match cached {
            Some(paint) => paint,
            None => {
                let paint = crate::ui::theme::Paint::load(host).await;
                self.watch().paint = Some(paint.clone());
                paint
            }
        };
        let lines = widget::lines(active, &paint, width);
        let _ = host
            .set_widget(widget::KEY, lines, Placement::AboveEditor)
            .await;
    }
}

const RUN: &str = "Run it";
const SKIP: &str = "Skip this task";
const STOP: &str = "Stop the workflow";

/// Asks about a pending approval until someone answers it here or in another pi session.
async fn ask_approval(app: App, host: Host, run: RunState) {
    let Some(asked) = run.approval.as_ref().map(|a| a.asked_ms) else {
        return;
    };
    let task = run
        .approval
        .as_ref()
        .map(|a| a.task.clone())
        .unwrap_or_default();
    let title = format!("Workflow {}: run \"{task}\"?", run.workflow.name);
    let open = async {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let pending = app
                .runs
                .get(&run.id)
                .and_then(|r| r.approval)
                .map(|a| a.asked_ms);
            if pending != Some(asked) {
                return;
            }
        }
    };
    let choice = tokio::select! {
        choice = host.select(&title, &[RUN, SKIP, STOP]) => choice,
        () = open => return,
    };
    let decision = match choice.ok().flatten().as_deref() {
        Some(RUN) => Decision::Run,
        Some(SKIP) => Decision::Skip,
        Some(STOP) => Decision::Stop,
        _ => return,
    };
    if let Err(err) = app.approve(&run.id, decision) {
        let _ = host.notify_warning(err).await;
    }
}

fn create_run_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir.join(control::COMMANDS_DIR))?;
    std::fs::write(dir.join(SCHEMA_FILE), REPORT_SCHEMA)?;
    std::fs::write(dir.join(planner::SCHEMA_FILE), planner::SCHEMA)?;
    std::fs::write(dir.join(MEMORY_FILE), "")
}

/// Starts `pi-rust-ext workflows --runner <dir>` in its own process group, without pipes to pi,
/// so it outlives this extension and pi.
fn spawn_runner(dir: &Path) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let log = std::fs::File::create(dir.join(LOG_FILE))?;
    let mut command = tokio::process::Command::new(exe);
    command
        .args(["workflows", runner::FLAG])
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    let mut child = command.spawn()?;
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(())
}

#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Built-in subagent types and the custom agent files of the subagents extension.
pub fn subagent_types(host: &Host) -> Vec<String> {
    let mut names: Vec<String> = ["explore", "build", "advisor"].map(String::from).to_vec();
    let mut dirs = vec![host.agent_dir().join("ext/subagents/agents")];
    if trusted(host) {
        dirs.push(host.project_file("ext/subagents/agents"));
    }
    for dir in dirs {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned());
            if path.extension().is_some_and(|e| e == "md")
                && let Some(name) = name.filter(|n| !names.contains(n))
            {
                names.push(name);
            }
        }
    }
    names
}
