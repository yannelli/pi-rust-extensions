//! The `list`, `status`, and `stop` actions, run details, and background completion messages.

use std::path::PathBuf;
use std::time::Duration;

use pi_ext_sdk::DeliverAs;
use pi_ext_sdk::prelude::*;
use tokio::task::JoinSet;

use crate::agent::AgentDef;
use crate::app::App;
use crate::jobs::Started;
use crate::render;
use crate::runs::{Run, Status};
use crate::store;

/// The custom message type of a finished `async` call, as pi-subagents names it.
pub const NOTIFY_TYPE: &str = "subagent-notify";
const DELIVERY_ATTEMPTS: u32 = 6;

pub fn list_agents(agents: &[AgentDef]) -> ToolResult {
    let lines: Vec<String> = agents
        .iter()
        .map(|a| {
            format!(
                "- {} ({}): {} [tools: {}; model: {}]",
                a.name,
                a.source.label(),
                a.description,
                a.tools_label(),
                a.model_label()
            )
        })
        .collect();
    ToolResult::text(lines.join("\n")).details(json!({ "agents": agents }))
}

pub fn status(app: &App, id: Option<&str>) -> ToolResult {
    let runs: Vec<Run> = match id {
        Some(id) => app.runs.get(id).into_iter().collect(),
        None => app.runs.all(),
    };
    if runs.is_empty() {
        return ToolResult::text(match id {
            Some(id) => format!("No run {id}."),
            None => "No subagent runs.".into(),
        });
    }
    let text = if id.is_some() {
        render::final_text(&runs)
    } else {
        render::progress_text(&runs)
    };
    ToolResult::text(text).details(run_details(&runs))
}

pub fn stop(app: &App, id: Option<&str>) -> ToolResult {
    match id {
        None => ToolResult::error("stop needs `id`."),
        Some(id) if app.runs.cancel(id) => ToolResult::text(format!("Stopping run {id}.")),
        Some(id) => ToolResult::error(format!("Run {id} is not running.")),
    }
}

/// Waits for the runs of one `async` call, rewrites `<asyncDir>/status.json` after each run ends,
/// and sends the `subagent-notify` message.
pub async fn report_when_done(
    app: App,
    host: Host,
    mode: &'static str,
    async_dir: PathBuf,
    started: Vec<Started>,
) {
    let ids: Vec<String> = started.iter().map(|s| s.id.clone()).collect();
    let mut pending = JoinSet::new();
    for s in started {
        pending.spawn(s.handle);
    }
    while pending.join_next().await.is_some() {
        store::write_status(&async_dir, &collect(&app, &ids));
    }
    let _ = app.draw_widget(&host).await;
    let runs = collect(&app, &ids);
    let message = CustomMessage::new(NOTIFY_TYPE, notify_text(&runs))
        .display(true)
        .details(call_details(&runs, mode))
        .deliver_as(DeliverAs::FollowUp)
        .trigger_turn(true);
    deliver_with_retry(&host, message, "subagents").await;
}

/// Sends `message`, retrying with backoff while pi does not accept it (for example during
/// `/reload`), then emits `agent:wake` so a sleeping agent sees it at once.
pub async fn deliver_with_retry(host: &Host, message: CustomMessage, source: &str) {
    let mut delay = Duration::from_millis(500);
    for attempt in 1..=DELIVERY_ATTEMPTS {
        match host.send_message(message.clone()).await {
            Ok(()) => {
                let wake = json!({ "source": source, "reason": "background results arrived" });
                let _ = host.emit(crate::messaging::WAKE_CHANNEL, wake).await;
                return;
            }
            Err(err) if attempt == DELIVERY_ATTEMPTS => {
                pi_ext_sdk::warn!("cannot deliver the subagent result: {}", err.message());
            }
            Err(_) => {
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
        }
    }
}

/// The header line, then per run its directory and session file, then the run results.
pub fn notify_text(runs: &[Run]) -> String {
    let header = match (runs, store::call_state(runs)) {
        ([run], _) => format!(
            "Background task {}: **{}** ({})",
            outcome(run.status),
            run.agent,
            run.id
        ),
        (_, "complete") => format!("Background tasks completed ({}):", runs.len()),
        (_, state) => format!("Background tasks {state} ({}):", runs.len()),
    };
    let mut lines = vec![header];
    for run in runs {
        lines.push(format!(
            "Retention-managed async directory: {}",
            run.dir().display()
        ));
        lines.push(format!("Session file: {}", run.session_file.display()));
    }
    let summary: Vec<String> = runs
        .iter()
        .map(|r| format!("{} {} ({})", r.id, r.status.as_str(), r.agent))
        .collect();
    format!(
        "{}\n\nBackground subagent run(s) finished: {}.\n\n{}",
        lines.join("\n"),
        summary.join(", "),
        render::final_text(runs)
    )
}

fn outcome(status: Status) -> &'static str {
    match status {
        Status::Cancelled => "stopped",
        Status::Completed => "completed",
        Status::Failed | Status::Running => "failed",
    }
}

pub fn collect(app: &App, ids: &[String]) -> Vec<Run> {
    ids.iter().filter_map(|id| app.runs.get(id)).collect()
}

/// `{ runs }` with one entry per run.
pub fn run_details(runs: &[Run]) -> Value {
    let runs: Vec<Value> = runs
        .iter()
        .map(|r| {
            json!({
                "id": r.id, "agent": r.agent, "task": r.task, "status": r.status,
                "background": r.background, "model": r.progress.model.clone().or_else(|| r.model.clone()),
                "turns": r.progress.turns, "tools": r.progress.tools, "usage": r.progress.usage,
                "output": r.progress.output(), "exitCode": r.exit_code,
                "durationMs": r.elapsed().as_millis() as u64, "sessionFile": r.session_file,
            })
        })
        .collect();
    json!({ "runs": runs })
}

/// [`run_details`] plus the fields of the pi-subagents result: `mode` (`single` or `parallel`),
/// `runId` for a single run, and `results` with one row per finished run.
pub fn call_details(runs: &[Run], mode: &str) -> Value {
    let mut details = run_details(runs);
    let results: Vec<Value> = runs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.status != Status::Running)
        .map(|(index, r)| {
            let mut row = json!({
                "index": index, "agent": r.agent, "success": r.status == Status::Completed,
                "sessionFile": r.session_file,
            });
            if let Some(code) = r.exit_code {
                row["exitCode"] = json!(code);
            }
            row
        })
        .collect();
    details["mode"] = json!(mode);
    details["results"] = json!(results);
    if let ("single", [run]) = (mode, runs) {
        details["runId"] = json!(run.id);
    }
    details
}
