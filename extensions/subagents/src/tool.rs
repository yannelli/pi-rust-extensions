//! The `subagent` tool: one task, parallel `tasks`, foreground or `async` background runs.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pi_ext_sdk::CancellationToken;
use pi_ext_sdk::prelude::*;
use serde::Deserialize;
use tokio::sync::{Notify, Semaphore};

use crate::actions::{collect, list_agents, report_when_done, run_details, status, stop};
use crate::agent::AgentDef;
use crate::app::App;
use crate::command::{Inherit, Overrides, pi_binary};
use crate::jobs::{self, Job, Launch, Started};
use crate::render;
use crate::runs::Status;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Params {
    pub action: Option<String>,
    pub agent: Option<String>,
    pub task: Option<String>,
    pub tasks: Option<Value>,
    #[serde(rename = "async")]
    pub background: Option<bool>,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub cwd: Option<String>,
    pub id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskParam {
    agent: Option<String>,
    task: String,
    model: Option<String>,
    thinking: Option<String>,
    cwd: Option<String>,
}

pub fn spec() -> ToolSpec {
    let task = json!({
        "type": "object",
        "properties": {
            "agent": { "type": "string", "description": "Agent name; defaults to the call's `agent`, else explore." },
            "task": { "type": "string" },
            "model": { "type": "string", "description": "provider/id for this task" },
            "thinking": { "type": "string" },
            "cwd": { "type": "string" }
        },
        "required": ["task"]
    });
    ToolSpec::new(
        "subagent",
        "Run a task in a child pi agent with its own context and tools. Agents: explore (read-only search), \
build (full tools), advisor (read-only review), plus custom agents. Pass `tasks` for parallel runs and \
`async: true` to run in the background; background results arrive as a message. Actions: run (default), \
list (agents), status (runs, or one run by `id`), stop (`id`).",
    )
    .label("Subagent")
    .parameters(json!({
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": ["run", "list", "status", "stop"] },
            "agent": { "type": "string", "description": "Agent name (default explore)" },
            "task": { "type": "string", "description": "The full task with the context the agent needs" },
            "tasks": { "type": "array", "items": task, "description": "Independent tasks to run in parallel" },
            "async": { "type": "boolean", "description": "Run in the background and report on completion" },
            "model": { "type": "string", "description": "provider/id; default: the agent's model, else this session's model" },
            "thinking": { "type": "string", "description": "off, minimal, low, medium, high, xhigh, max" },
            "cwd": { "type": "string", "description": "Working directory; default: the session cwd" },
            "id": { "type": "string", "description": "Run id for status and stop" }
        }
    }))
    .prompt_snippet("Delegate tasks to child agents (explore, build, advisor, custom), in parallel or in the background")
    .guideline("Use subagent explore for broad codebase searches and subagent advisor for a second opinion on a plan or change.")
    .guideline("Subagents do not see this conversation: write each task with the paths, goals, and constraints it needs.")
    .guideline("Run independent subagent tasks together with `tasks`; use `async: true` only when you can continue without the result.")
}

pub async fn execute(app: App, params: Params, ctx: ToolCtx) -> Result<ToolResult> {
    let host = ctx.host().clone();
    match params.action.as_deref().unwrap_or("run") {
        "list" => Ok(list_agents(&app.agents(&host))),
        "status" => Ok(status(&app, params.id.as_deref())),
        "stop" => Ok(stop(&app, params.id.as_deref())),
        "run" => run(app, params, ctx).await,
        other => Ok(ToolResult::error(format!(
            "Unknown action \"{other}\". Use run, list, status, or stop."
        ))),
    }
}

fn jobs_of(
    params: &Params,
    agents: &[AgentDef],
    cwd: &Path,
) -> std::result::Result<Vec<Job>, String> {
    let tasks: Vec<TaskParam> = match &params.tasks {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(text)) => {
            serde_json::from_str(text).map_err(|e| format!("tasks: {e}"))?
        }
        Some(value) => serde_json::from_value(value.clone()).map_err(|e| format!("tasks: {e}"))?,
    };
    let mut wanted: Vec<TaskParam> = tasks;
    if wanted.is_empty() {
        let task = params
            .task
            .clone()
            .filter(|t| !t.trim().is_empty())
            .ok_or("Pass `task` or `tasks`.")?;
        wanted.push(TaskParam {
            agent: None,
            task,
            model: None,
            thinking: None,
            cwd: None,
        });
    }
    let default_agent = params.agent.clone().unwrap_or_else(|| "explore".into());
    wanted
        .into_iter()
        .map(|t| {
            let name = t.agent.unwrap_or_else(|| default_agent.clone());
            let agent = agents
                .iter()
                .find(|a| a.name == name)
                .cloned()
                .ok_or_else(|| {
                    let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
                    format!("Unknown agent \"{name}\". Available: {}.", names.join(", "))
                })?;
            let dir = t
                .cwd
                .or_else(|| params.cwd.clone())
                .map_or_else(|| cwd.to_path_buf(), |d| cwd.join(d));
            Ok(Job {
                agent,
                task: t.task,
                overrides: Overrides {
                    model: t
                        .model
                        .or_else(|| params.model.clone())
                        .filter(|m| !m.is_empty()),
                    thinking: t
                        .thinking
                        .or_else(|| params.thinking.clone())
                        .filter(|m| !m.is_empty()),
                },
                cwd: dir,
            })
        })
        .collect()
}

async fn run(app: App, params: Params, ctx: ToolCtx) -> Result<ToolResult> {
    let host = ctx.host().clone();
    let config = app.config(&host);
    let cwd = ctx.snapshot().cwd_path();
    let cwd = if cwd.as_os_str().is_empty() {
        host.cwd()
    } else {
        cwd
    };
    let jobs = match jobs_of(&params, &app.agents(&host), &cwd) {
        Ok(jobs) if jobs.len() > config.max_tasks => {
            return Ok(ToolResult::error(format!(
                "At most {} tasks per call.",
                config.max_tasks
            )));
        }
        Ok(jobs) => jobs,
        Err(message) => return Ok(ToolResult::error(message)),
    };
    let background = params.background.unwrap_or(config.async_by_default);
    let launch = Launch {
        program: pi_binary(config.pi_bin.as_deref()),
        parent: Inherit {
            model: ctx.snapshot().model_ref(),
            thinking: ctx.snapshot().thinking_level.clone(),
        },
        extra_args: config.extra_args.clone(),
        keep: config.max_recent_runs,
        limit: Arc::new(Semaphore::new(config.max_parallel)),
        changed: Arc::new(Notify::new()),
    };
    let started: Vec<Started> = jobs
        .into_iter()
        .map(|job| {
            let cancel = if background {
                CancellationToken::new()
            } else {
                ctx.cancel_token().child_token()
            };
            jobs::start(&app, &host, &launch, job, background, cancel)
        })
        .collect();
    let ids: Vec<String> = started.iter().map(|s| s.id.clone()).collect();
    if background {
        app.start_widget(&host);
        tokio::spawn(report_when_done(app.clone(), host, started));
        let text = format!(
            "Started {} background subagent run(s): {}. The results arrive as a message when they finish; \
use action status with an id to check progress.",
            ids.len(),
            ids.join(", ")
        );
        return Ok(ToolResult::text(text).details(run_details(&collect(&app, &ids))));
    }
    let mut handles: Vec<_> = started.into_iter().map(|s| s.handle).collect();
    let mut last_update = String::new();
    while !handles.iter().all(|h| h.is_finished()) {
        tokio::select! {
            _ = launch.changed.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
        let runs = collect(&app, &ids);
        let text = render::progress_text(&runs);
        if text != last_update {
            ctx.update(ToolResult::text(text.clone()).details(run_details(&runs)))?;
            last_update = text;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    for handle in handles.drain(..) {
        let _ = handle.await;
    }
    let runs = collect(&app, &ids);
    let failed = runs.iter().all(|r| r.status != Status::Completed);
    let result = ToolResult::text(render::final_text(&runs)).details(run_details(&runs));
    Ok(if failed { result.failed() } else { result })
}
