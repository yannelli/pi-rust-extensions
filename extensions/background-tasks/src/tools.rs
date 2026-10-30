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
        "Start a named long-running shell command in the background and return at once with a task ID and output path. Completed, failed, or killed state arrives as a <background-task-notification> that starts a follow-up turn; do not sleep or poll to wait. Model-visible logs are bounded to 50KB.",
    )
    .label("Background Run")
    .prompt_snippet("Start long-running shell commands (tests, builds, servers, watchers) in the background")
    .guideline("Use bg_run instead of bash for commands expected to run for a long time, such as test suites, dev servers, watchers, or builds.")
    .guideline("Set name to a concise 2-6 word label for the task dock, not the raw command.")
    .guideline("After bg_run, continue only independent work or end the turn; the terminal notification wakes you. Do not call sleep, bg_status, or bg_logs to wait.")
    .guideline("Treat <background-task-notification> as terminal truth; call bg_logs only when the output is needed.")
    .parameters(json!({
        "type": "object",
        "properties": {
            "name": { "type": "string", "description": "Short human-readable task name shown in the task dock. Use 2-6 words, not the raw command." },
            "command": { "type": "string", "description": "Shell command to start in the background" },
            "isAgent": { "type": "boolean", "description": "Set true only when the task launches an LLM/agent process such as a child `pi -p ...`; false for scripts, tests, servers, sleeps, and ordinary shell commands." },
            "description": { "type": "string", "description": "Optional longer human-readable context for the task" },
            "timeoutSeconds": { "type": "number", "description": "Optional timeout; the task is stopped and marked timed out when exceeded" },
            "notifyOnCompletion": { "type": "boolean", "description": "Deliver the terminal notification. Default: true." },
            "triggerOnCompletion": { "type": "boolean", "description": "Let that notification start a follow-up agent turn. Default: true; requires notifyOnCompletion." },
            "surviveReload": { "type": "boolean", "description": "Keep the task running across /reload. Default: true. Tasks stop when the session quits or switches." }
        },
        "required": ["name", "command", "isAgent"]
    }))
}

fn status_spec() -> ToolSpec {
    ToolSpec::new("bg_status", "Inspect one background task or list all running and recent background tasks. Point-in-time inspection, not a waiting primitive.")
        .label("Background Status")
        .prompt_snippet("Inspect point-in-time status of background tasks; do not poll it as a wait loop")
        .guideline("Use bg_status when the user asks for an update, notification was disabled, or a task looks hung.")
        .parameters(json!({
            "type": "object",
            "properties": { "taskId": { "type": "string", "description": "Optional task ID or unambiguous prefix. Omit to list all tasks." } }
        }))
}

fn logs_spec() -> ToolSpec {
    ToolSpec::new("bg_logs", "Read bounded output from a background task for deliberate inspection; not a waiting primitive. Output is capped at 50KB and points to the full output file when truncated.")
        .label("Background Logs")
        .prompt_snippet("Read bounded task output when needed; do not tail it repeatedly to wait")
        .guideline("Use bg_logs with a modest maxBytes value only when task output is needed.")
        .parameters(json!({
            "type": "object",
            "properties": {
                "taskId": { "type": "string", "description": "Task ID or unambiguous prefix" },
                "maxBytes": { "type": "number", "description": "Maximum bytes to return, capped at 50KB. Default: 50KB." },
                "tail": { "type": "boolean", "description": "Read the tail of the log when true, the head when false. Default: true." }
            },
            "required": ["taskId"]
        }))
}

fn kill_spec() -> ToolSpec {
    ToolSpec::new("bg_kill", "Stop a running background task by ID. Fails when the task is unknown or already finished.")
        .label("Background Kill")
        .prompt_snippet("Stop a running background task by ID")
        .guideline("Use bg_kill when the user asks to stop a background task or a bg_run command is no longer needed.")
        .parameters(json!({
            "type": "object",
            "properties": { "taskId": { "type": "string", "description": "Task ID or unambiguous prefix to stop" } },
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
