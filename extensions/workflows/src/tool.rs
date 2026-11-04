//! The `workflow` tool: agents list, read, save, delete, run, and inspect workflows.

use std::collections::BTreeMap;
use std::time::Duration;

use pi_ext_sdk::prelude::*;
use serde::Deserialize;

use crate::app::App;
use crate::model::{Workflow, format_duration, slug};
use crate::runs::{Origin, RunState};
use crate::store::Source;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Params {
    pub action: String,
    pub name: Option<String>,
    pub workflow: Option<Value>,
    pub inputs: Option<BTreeMap<String, String>>,
    pub scope: Option<String>,
    pub id: Option<String>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new(
        "workflow",
        "Manage and run pi workflows: multi-step pipelines where each step runs as its own agent with its \
own model, thinking level, tools, skills, MCP servers, subagent rules, and environment, and tasks can wait, \
repeat, loop until a goal is met, or ask the user first. Actions: list; get (name); save (workflow, scope); \
delete (name); run (name or workflow, inputs); status (all runs, or one run by id); stop (id); resume (id). \
The workflow-create and workflow-optimize skills describe the format and how to design workflows.",
    )
    .label("Workflow")
    .parameters(json!({
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": ["list", "get", "save", "delete", "run", "status", "stop", "resume"] },
            "name": { "type": "string", "description": "Workflow name for get, delete, and run" },
            "workflow": { "type": "object", "description": "A full workflow definition for save, or an unsaved workflow for run" },
            "inputs": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Values for the {{placeholders}} of the prompts; `goal` for dynamic workflows" },
            "scope": { "type": "string", "enum": ["global", "project"], "description": "Where save writes; default: where the workflow is now, else global" },
            "id": { "type": "string", "description": "Run id for status, stop, and resume" }
        },
        "required": ["action"]
    }))
    .prompt_snippet("Create, edit, run, and inspect multi-agent workflows (implement, review, ship, deploy, monitor)")
    .guideline("Load the workflow-create skill before you design a workflow, and workflow-optimize before you tune one.")
    .guideline("A workflow run continues in the background; its result arrives as a message when it ends.")
}

pub async fn execute(app: App, params: Params, ctx: ToolCtx) -> Result<ToolResult> {
    let host = ctx.host().clone();
    let result = match params.action.as_str() {
        "list" => Ok(list_text(&app, &host)),
        "get" => get(&app, &host, &params),
        "save" => save(&app, &host, &params),
        "delete" => delete(&app, &host, &params),
        "run" => run(&app, &host, params).await,
        "status" => status(&app, params.id.as_deref()),
        "stop" => with_id(&params, |id| {
            if app.stop(id) {
                Ok(format!("Stopping run {id}."))
            } else {
                Err(format!("Run {id} is not active."))
            }
        }),
        "resume" => match params.id.as_deref() {
            Some(id) => app
                .resume(&host, id)
                .await
                .map(|()| format!("Resumed run {id}.")),
            None => Err("resume needs an id".into()),
        },
        other => Err(format!("unknown action {other:?}")),
    };
    Ok(match result {
        Ok(text) => ToolResult::text(text),
        Err(text) => ToolResult::error(text),
    })
}

type Answer = std::result::Result<String, String>;

fn with_id(params: &Params, act: impl FnOnce(&str) -> Answer) -> Answer {
    params
        .id
        .as_deref()
        .map_or_else(|| Err(format!("{} needs an id", params.action)), act)
}

fn name_of(params: &Params) -> std::result::Result<&str, String> {
    params
        .name
        .as_deref()
        .ok_or_else(|| format!("{} needs a name", params.action))
}

pub fn list_text(app: &App, host: &Host) -> String {
    let (workflows, errors) = app.dirs(host).load();
    let mut lines: Vec<String> = workflows
        .iter()
        .map(|s| {
            let w = &s.workflow;
            let inputs = w.inputs();
            let inputs = if inputs.is_empty() {
                String::new()
            } else {
                format!(", inputs: {}", inputs.join(", "))
            };
            format!(
                "- {} ({}, {}, {} steps{inputs}): {}",
                w.name,
                s.source.label(),
                w.mode.label(),
                w.steps.len(),
                w.description
            )
        })
        .collect();
    if lines.is_empty() {
        lines.push("No workflows yet.".into());
    }
    lines.extend(errors.into_iter().map(|e| format!("Unreadable: {e}")));
    lines.join("\n")
}

fn get(app: &App, host: &Host, params: &Params) -> Answer {
    let name = name_of(params)?;
    let stored = app
        .dirs(host)
        .find(name)
        .ok_or_else(|| format!("no workflow {name:?}"))?;
    let text = serde_json::to_string_pretty(&stored.workflow).map_err(|e| e.to_string())?;
    Ok(format!(
        "{} ({})\n{text}",
        stored.path.display(),
        stored.source.label()
    ))
}

fn save(app: &App, host: &Host, params: &Params) -> Answer {
    let value = params.workflow.clone().ok_or("save needs a workflow")?;
    let workflow: Workflow =
        serde_json::from_value(value).map_err(|e| format!("invalid workflow: {e}"))?;
    let dirs = app.dirs(host);
    let source = match params.scope.as_deref() {
        Some("project") => Source::Project,
        Some("global") => Source::Global,
        _ => dirs
            .find(&workflow.name)
            .map_or(Source::Global, |s| s.source),
    };
    let path = dirs.save(&workflow, source)?;
    let inputs = workflow.inputs();
    let inputs = if inputs.is_empty() {
        String::new()
    } else {
        format!(" Inputs: {}.", inputs.join(", "))
    };
    Ok(format!(
        "Saved {} to {}.{inputs}",
        slug(&workflow.name),
        path.display()
    ))
}

fn delete(app: &App, host: &Host, params: &Params) -> Answer {
    let name = name_of(params)?;
    let stored = app
        .dirs(host)
        .find(name)
        .ok_or_else(|| format!("no workflow {name:?}"))?;
    std::fs::remove_file(&stored.path).map_err(|e| e.to_string())?;
    Ok(format!("Deleted {}.", stored.path.display()))
}

async fn run(app: &App, host: &Host, params: Params) -> Answer {
    let workflow = match (&params.workflow, &params.name) {
        (Some(value), _) => {
            serde_json::from_value(value.clone()).map_err(|e| format!("invalid workflow: {e}"))?
        }
        (None, Some(name)) => {
            app.dirs(host)
                .find(name)
                .ok_or_else(|| format!("no workflow {name:?}"))?
                .workflow
        }
        (None, None) => return Err("run needs a name or a workflow".into()),
    };
    let inputs = params.inputs.unwrap_or_default();
    let id = app.start(host, workflow, inputs, Origin::Agent).await?;
    Ok(format!(
        "Started run {id}. It runs in the background; its result arrives as a message. Use action status with this id to check it."
    ))
}

fn status(app: &App, id: Option<&str>) -> Answer {
    let Some(id) = id else {
        return Ok(runs_text(app));
    };
    let run = app.runs.get(id).ok_or_else(|| format!("no run {id}"))?;
    Ok(detail(&run))
}

pub fn runs_text(app: &App) -> String {
    let runs = app.runs.all();
    if runs.is_empty() {
        return "No workflow runs.".into();
    }
    let lines: Vec<String> = runs
        .iter()
        .map(|r| {
            format!(
                "- {} {} · {} · {}",
                r.id,
                r.workflow.name,
                r.status.label(),
                r.stats()
            )
        })
        .collect();
    lines.join("\n")
}

/// Everything about one run, for people and for optimizing the workflow.
pub fn detail(run: &RunState) -> String {
    let mut lines = vec![format!(
        "Run {} · {} · {} · {}",
        run.id,
        run.workflow.name,
        run.status.label(),
        run.stats()
    )];
    if !run.inputs.is_empty() {
        let inputs: Vec<String> = run.inputs.iter().map(|(k, v)| format!("{k}={v}")).collect();
        lines.push(format!("Inputs: {}", inputs.join("; ")));
    }
    if !run.note.is_empty() {
        lines.push(format!("Now: {}", run.note));
    }
    if let Some(error) = &run.error {
        lines.push(format!("Error: {error}"));
    }
    for (i, (step, state)) in run.workflow.steps.iter().zip(&run.steps).enumerate() {
        let time = state
            .elapsed()
            .map(|d| format_duration(Duration::from_secs(d.as_secs())))
            .unwrap_or_default();
        let model = state.model.as_deref().unwrap_or("-");
        lines.push(format!(
            "{}. {} · {} · {time} · ${:.2} · {} agent runs · {} turns · {model}",
            i + 1,
            step.name,
            state.status.label(),
            state.cost,
            state.runs,
            state.turns
        ));
        lines.extend(state.log.iter().map(|l| format!("   - {l}")));
        if !state.summary.is_empty() {
            lines.push(format!(
                "   Report: {}",
                state.summary.replace('\n', "\n   ")
            ));
        }
    }
    lines.join("\n")
}
