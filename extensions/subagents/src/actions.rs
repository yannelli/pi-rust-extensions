//! The `list`, `status`, and `stop` actions, run details, and background completion messages.

use pi_ext_sdk::DeliverAs;
use pi_ext_sdk::prelude::*;

use crate::agent::AgentDef;
use crate::app::App;
use crate::jobs::Started;
use crate::render;
use crate::runs::Run;

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

pub async fn report_when_done(app: App, host: Host, started: Vec<Started>) {
    let ids: Vec<String> = started.iter().map(|s| s.id.clone()).collect();
    for s in started {
        let _ = s.handle.await;
    }
    let _ = app.draw_widget(&host).await;
    let runs = collect(&app, &ids);
    let summary: Vec<String> = runs
        .iter()
        .map(|r| format!("{} {} ({})", r.id, r.status.as_str(), r.agent))
        .collect();
    let content = format!(
        "Background subagent run(s) finished: {}.\n\n{}",
        summary.join(", "),
        render::final_text(&runs)
    );
    let message = CustomMessage::new("subagent-result", content)
        .display(true)
        .details(run_details(&runs))
        .deliver_as(DeliverAs::FollowUp)
        .trigger_turn(true);
    if let Err(err) = host.send_message(message).await {
        pi_ext_sdk::warn!("cannot deliver the subagent result: {}", err.message());
    }
}

pub fn collect(app: &App, ids: &[String]) -> Vec<Run> {
    ids.iter().filter_map(|id| app.runs.get(id)).collect()
}

pub fn run_details(runs: &[Run]) -> Value {
    let runs: Vec<Value> = runs
        .iter()
        .map(|r| {
            json!({
                "id": r.id, "agent": r.agent, "task": r.task, "status": r.status,
                "background": r.background, "model": r.progress.model.clone().or_else(|| r.model.clone()),
                "turns": r.progress.turns, "tools": r.progress.tools, "usage": r.progress.usage,
                "output": r.progress.output(), "exitCode": r.exit_code,
                "durationMs": r.elapsed().as_millis() as u64,
            })
        })
        .collect();
    json!({ "runs": runs })
}
