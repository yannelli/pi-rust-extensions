use std::sync::Arc;

use pi_ext_sdk::prelude::*;
use serde::Deserialize;

use crate::registry::{Registry, StartOptions};
use crate::task::{self, Task};
use crate::{ensure, ui};

pub const MAX_LOG_BYTES: u64 = 50 * 1024;
pub const DEFAULT_LOG_BYTES: u64 = 50 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunArgs {
    #[serde(default)]
    pub name: Option<String>,
    pub command: String,
    #[serde(default)]
    pub is_agent: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub timeout_seconds: Option<f64>,
    #[serde(default)]
    pub notify_on_completion: Option<bool>,
    #[serde(default)]
    pub trigger_on_completion: Option<bool>,
    #[serde(default)]
    pub survive_reload: Option<bool>,
}

impl RunArgs {
    pub fn into_options(self) -> StartOptions {
        let notify = self.notify_on_completion.unwrap_or(true);
        StartOptions {
            name: self.name,
            command: self.command,
            description: self.description,
            is_agent: self.is_agent,
            timeout_seconds: self
                .timeout_seconds
                .filter(|s| *s > 0.0)
                .map(|s| s.ceil() as u64),
            notify_on_completion: notify,
            trigger_on_completion: notify && self.trigger_on_completion.unwrap_or(true),
            survive_reload: self.survive_reload.unwrap_or(true),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusArgs {
    #[serde(default)]
    task_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogsArgs {
    task_id: String,
    #[serde(default)]
    max_bytes: Option<f64>,
    #[serde(default)]
    tail: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KillArgs {
    task_id: String,
}

pub fn normalize_max_bytes(value: Option<f64>) -> u64 {
    match value {
        Some(v) if v.is_finite() && v >= 1.0 => (v as u64).min(MAX_LOG_BYTES),
        _ => DEFAULT_LOG_BYTES,
    }
}

pub fn started_text(task: &Task) -> String {
    let mut text = format!(
        "Started background task {} ({}).\nOutput: {}",
        task.name,
        task.id,
        task.output_path().display()
    );
    if task.notify_on_completion {
        text.push_str("\nA <background-task-notification> arrives when it finishes; do not poll.");
    }
    text
}

/// Bounded output of one task with a header line.
pub fn logs_text(task: &Task, max_bytes: u64, tail: bool) -> std::io::Result<String> {
    let slice = task::read_bounded(&task.output_path(), max_bytes, tail)?;
    let mut text = format!(
        "{} ({}) [{}] {} of {} ({})",
        task.name,
        task.id,
        task.status.label(),
        task::format_size(slice.bytes_read),
        task::format_size(slice.total),
        if tail { "tail" } else { "head" }
    );
    if slice.truncated {
        text.push_str(&format!(
            "\nTruncated; full output: {}",
            task.output_path().display()
        ));
    }
    text.push('\n');
    text.push_str(if slice.text.is_empty() {
        "(no output yet)"
    } else {
        &slice.text
    });
    Ok(text)
}

fn run_spec() -> ToolSpec {
    ToolSpec::new(
        "bg_run",
        "Start a long-running shell command and return its task ID and output path immediately.",
    )
    .label("Background Run")
    .parameters(json!({
        "type": "object",
        "properties": {
            "name": { "type": "string", "description": "2-6 word dock label" },
            "command": { "type": "string" },
            "isAgent": { "type": "boolean", "description": "True for LLM/agent processes" },
            "description": { "type": "string" },
            "timeoutSeconds": { "type": "number", "description": "Stop after this duration" },
            "notifyOnCompletion": { "type": "boolean", "default": true },
            "triggerOnCompletion": { "type": "boolean", "default": true, "description": "Notification starts a turn; requires notifyOnCompletion" },
            "surviveReload": { "type": "boolean", "default": true }
        },
        "required": ["name", "command", "isAgent"]
    }))
}

fn status_spec() -> ToolSpec {
    ToolSpec::new("bg_status", "Inspect tasks when asked, notifications are off, or a task seems hung.")
        .label("Background Status")
        .parameters(json!({
            "type": "object",
            "properties": { "taskId": { "type": "string", "description": "ID/unique prefix; omit for all" } }
        }))
}

fn logs_spec() -> ToolSpec {
    ToolSpec::new("bg_logs", "Read task output when needed; truncation includes the full file path.")
        .label("Background Logs")
        .parameters(json!({
            "type": "object",
            "properties": {
                "taskId": { "type": "string", "description": "ID/unique prefix" },
                "maxBytes": { "type": "number", "maximum": MAX_LOG_BYTES, "default": DEFAULT_LOG_BYTES },
                "tail": { "type": "boolean", "default": true, "description": "False reads head" }
            },
            "required": ["taskId"]
        }))
}

fn kill_spec() -> ToolSpec {
    ToolSpec::new(
        "bg_kill",
        "Stop a running task when requested or no longer needed.",
    )
    .label("Background Kill")
    .parameters(json!({
        "type": "object",
        "properties": { "taskId": { "type": "string", "description": "ID/unique prefix" } },
        "required": ["taskId"]
    }))
}

pub fn register(ext: Extension, registry: Arc<Registry>) -> Extension {
    let (r1, r2, r3, r4) = (
        registry.clone(),
        registry.clone(),
        registry.clone(),
        registry,
    );
    ext.tool(run_spec(), move |args: RunArgs, ctx: ToolCtx| {
        let registry = r1.clone();
        async move {
            ensure(ctx.host(), ctx.snapshot(), &registry);
            let cwd = ctx.snapshot().cwd.clone();
            let cwd = if cwd.is_empty() {
                ctx.host().cwd().display().to_string()
            } else {
                cwd
            };
            match registry.start(args.into_options(), &cwd) {
                Ok(task) => {
                    let _ = ui::refresh(ctx.host(), &registry).await;
                    Ok(ToolResult::text(started_text(&task)).details(&task))
                }
                Err(err) => Ok(ToolResult::error(err)),
            }
        }
    })
    .tool(status_spec(), move |args: StatusArgs, ctx: ToolCtx| {
        let registry = r2.clone();
        async move {
            ensure(ctx.host(), ctx.snapshot(), &registry);
            let tasks = match args.task_id.as_deref().filter(|id| !id.trim().is_empty()) {
                Some(id) => match registry.resolve(id) {
                    Ok(task) => vec![task],
                    Err(err) => return Ok(ToolResult::error(err)),
                },
                None => registry.tasks(),
            };
            Ok(ToolResult::text(ui::list_text(&tasks)).details(json!({ "tasks": tasks })))
        }
    })
    .tool(logs_spec(), move |args: LogsArgs, ctx: ToolCtx| {
        let registry = r3.clone();
        async move {
            ensure(ctx.host(), ctx.snapshot(), &registry);
            let task = match registry.resolve(&args.task_id) {
                Ok(task) => task,
                Err(err) => return Ok(ToolResult::error(err)),
            };
            let tail = args.tail.unwrap_or(true);
            let max = normalize_max_bytes(args.max_bytes);
            let text = logs_text(&task, max, tail)?;
            let details = json!({ "task": task, "path": task.output_path(), "tail": tail });
            Ok(ToolResult::text(text).details(details))
        }
    })
    .tool(kill_spec(), move |args: KillArgs, ctx: ToolCtx| {
        let registry = r4.clone();
        async move {
            ensure(ctx.host(), ctx.snapshot(), &registry);
            match registry.stop(&args.task_id, "stopped by bg_kill").await {
                Ok(task) => {
                    let _ = ui::refresh(ctx.host(), &registry).await;
                    let text = format!(
                        "Killed background task {} ({}). Output: {}",
                        task.name,
                        task.id,
                        task.output_path().display()
                    );
                    Ok(ToolResult::text(text).details(&task))
                }
                Err(err) => Ok(ToolResult::error(err)),
            }
        }
    })
}
