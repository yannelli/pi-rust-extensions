//! Shared extension state and the actions that the tool and the UI both use: start, resume,
//! stop, and the widget.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pi_ext_sdk::prelude::*;

use crate::launch::{Catalog, REPORT_SCHEMA, pi_binary};
use crate::model::Workflow;
use crate::runner::{self, MEMORY_FILE, SCHEMA_FILE};
use crate::runs::{Control, Origin, RunState, RunStatus, Runs, StepStatus, new_id, now_ms};
use crate::store::{self, Dirs};
use crate::ui::widget;

#[derive(Clone, Default)]
pub struct App {
    pub runs: Runs,
    ticking: Arc<AtomicBool>,
}

pub fn trusted(host: &Host) -> bool {
    host.snapshot().is_some_and(|s| s.is_project_trusted)
}

impl App {
    pub fn dirs(&self, host: &Host) -> Dirs {
        Dirs::new(&host.agent_dir(), &host.project_dir(), trusted(host))
    }

    pub fn runs_root(&self, host: &Host) -> PathBuf {
        store::runs_dir(&host.agent_dir())
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
            configs.push(host.project_dir().join("mcp.json"));
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

    /// Starts a run in the background and returns its id.
    pub async fn start(
        &self,
        host: &Host,
        workflow: Workflow,
        inputs: BTreeMap<String, String>,
        origin: Origin,
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
            ..RunState::default()
        };
        self.runs.insert(state, dir.clone(), None);
        self.spawn(host, &id, dir, catalog);
        Ok(id)
    }

    /// Continues a stopped, failed, or interrupted run at the task that did not finish.
    pub async fn resume(&self, host: &Host, id: &str) -> std::result::Result<(), String> {
        let run = self.runs.get(id).ok_or_else(|| format!("no run {id}"))?;
        if run.status.active() {
            return Err(format!("run {id} is still {}", run.status.label()));
        }
        if run.status == RunStatus::Done {
            return Err(format!("run {id} is done"));
        }
        let dir = self.runs.dir(id).ok_or_else(|| format!("no run {id}"))?;
        self.runs.update(id, |run| {
            run.status = RunStatus::Running;
            run.error = None;
            run.ended_ms = None;
            for step in &mut run.steps {
                if step.status == StepStatus::Failed {
                    step.status = StepStatus::Pending;
                }
            }
        });
        let catalog = self.catalog(host).await;
        self.spawn(host, id, dir, catalog);
        Ok(())
    }

    fn spawn(&self, host: &Host, id: &str, dir: PathBuf, catalog: Catalog) {
        let control = Control::default();
        self.runs.set_control(id, Some(control.clone()));
        self.runs.persist(id);
        let ctx = runner::Ctx {
            host: host.clone(),
            runs: self.runs.clone(),
            pi: pi_binary(),
            catalog,
            control,
            dir,
        };
        tokio::spawn(runner::drive(ctx, id.to_owned()));
        self.start_widget(host);
    }

    pub fn stop(&self, id: &str) -> bool {
        self.runs.control(id).map(|c| c.cancel.cancel()).is_some()
    }

    /// Stops the active runs as interrupted, so `resume` can continue them in a later pi.
    pub async fn shutdown(&self) {
        let active: Vec<String> = self
            .runs
            .all()
            .into_iter()
            .filter(|r| r.status.active())
            .map(|r| r.id)
            .collect();
        for id in &active {
            if let Some(control) = self.runs.control(id) {
                control.interrupted.store(true, Ordering::SeqCst);
                control.cancel.cancel();
            }
        }
        let deadline = tokio::time::Instant::now() + Duration::from_millis(1400);
        while self.runs.any_active() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Ends the current wait of a run at once.
    pub fn skip_wait(&self, id: &str) -> bool {
        let waiting = self.runs.get(id).is_some_and(|r| r.wait_until_ms.is_some());
        if let Some(control) = self.runs.control(id).filter(|_| waiting) {
            control.skip.notify_waiters();
            return true;
        }
        false
    }

    /// Redraws the widget each second while a run is active.
    pub fn start_widget(&self, host: &Host) {
        if !host.has_ui() || self.ticking.swap(true, Ordering::SeqCst) {
            return;
        }
        let app = self.clone();
        let host = host.clone();
        tokio::spawn(async move {
            let paint = crate::ui::theme::Paint::load(&host).await;
            loop {
                let active: Vec<RunState> = app
                    .runs
                    .all()
                    .into_iter()
                    .filter(|r| r.status.active())
                    .collect();
                if active.is_empty() {
                    let _ = host.clear_widget(widget::KEY).await;
                    break;
                }
                let width = host
                    .surface_width(&format!("widget:{}", widget::KEY))
                    .unwrap_or(100);
                let _ = host
                    .set_widget(
                        widget::KEY,
                        widget::lines(&active, &paint, width),
                        Placement::AboveEditor,
                    )
                    .await;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            app.ticking.store(false, Ordering::SeqCst);
            if app.runs.any_active() {
                app.start_widget(&host);
            }
        });
    }
}

fn create_run_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(SCHEMA_FILE), REPORT_SCHEMA)?;
    std::fs::write(dir.join(MEMORY_FILE), "")
}

/// Built-in subagent types and the custom agent files of the subagents extension.
pub fn subagent_types(host: &Host) -> Vec<String> {
    let mut names: Vec<String> = ["explore", "build", "advisor"].map(String::from).to_vec();
    let mut dirs = vec![host.agent_dir().join("ext/subagents/agents")];
    if trusted(host) {
        dirs.push(host.project_dir().join("ext/subagents/agents"));
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
