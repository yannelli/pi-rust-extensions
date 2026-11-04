//! The `simple_goal` tool and the `/simple-goal` command.

use pi_ext_sdk::prelude::*;
use serde::Deserialize;

use crate::state::{App, ENTRY, Goal, Status, describe};

pub const TOOL: &str = "simple_goal";

pub fn tool_spec() -> ToolSpec {
    ToolSpec::new(
        TOOL,
        "Use for small, well-defined tasks that should keep going until an evaluator agrees they are done; the evaluator checks after each run. `set` replaces the current simple goal, `status` reports it, `clear` removes it (also when the objective is impossible or the user should decide).",
    )
    .label("Simple goal")
    .parameters(json!({ "type": "object", "properties": {
        "action": { "type": "string", "enum": ["set", "status", "clear"] },
        "objective": { "type": "string", "description": "What must be true when the work is done. Required for set." },
        "done_when": { "type": "string", "description": "Optional check the evaluator uses, such as a command that must pass." }
    }, "required": ["action"] }))
}

#[derive(Deserialize)]
pub struct Args {
    action: String,
    #[serde(default)]
    objective: Option<String>,
    #[serde(default)]
    done_when: Option<String>,
}

pub async fn run_tool(app: App, args: Args, ctx: ToolCtx) -> Result<ToolResult> {
    let host = ctx.host();
    match args.action.as_str() {
        "set" => {
            let Some(objective) = args.objective.filter(|o| !o.trim().is_empty()) else {
                return Ok(ToolResult::error("set needs a non-empty objective."));
            };
            let goal = Goal::new(objective.trim().to_owned(), args.done_when);
            app.save(host, Some(goal)).await?;
            Ok(ToolResult::text(
                "Simple goal set. An evaluator checks it after each run and continues the work until it is done.",
            ))
        }
        "status" => {
            let max_rounds = app.config(host).await.max_rounds;
            Ok(ToolResult::text(describe(app.goal().as_ref(), max_rounds)))
        }
        "clear" => Ok(ToolResult::text(match clear(&app, host).await? {
            true => "Simple goal cleared.",
            false => "No simple goal.",
        })),
        other => Ok(ToolResult::error(format!(
            "Unknown action {other:?}; use set, status, or clear."
        ))),
    }
}

async fn clear(app: &App, host: &Host) -> Result<bool> {
    if app.goal().is_none() {
        return Ok(false);
    }
    app.save(host, None).await?;
    Ok(true)
}

pub const COMMAND: &str = "simple-goal";
pub const COMMAND_HELP: &str =
    "Set a simple goal and start work; no argument shows status; clear or resume.";

pub async fn run_command(app: App, args: String, ctx: CommandCtx) -> Result<()> {
    let host = ctx.host();
    match args.trim() {
        "" => {
            let max_rounds = app.config(host).await.max_rounds;
            host.notify_info(describe(app.goal().as_ref(), max_rounds))
                .await
        }
        "clear" => match clear(&app, host).await? {
            true => host.notify_info("simple goal cleared").await,
            false => host.notify_info("no simple goal").await,
        },
        "resume" => {
            let Some(mut goal) = app.goal() else {
                return host.notify_info("no simple goal").await;
            };
            if goal.status == Status::Active {
                return host.notify_info("simple goal is already active").await;
            }
            goal.resume();
            let text = format!("Resume the simple goal.\nObjective: {}", goal.objective);
            app.save(host, Some(goal)).await?;
            start_turn(host, text).await
        }
        objective => {
            let goal = Goal::new(objective.to_owned(), None);
            app.save(host, Some(goal)).await?;
            start_turn(host, format!("Simple goal: {objective}")).await
        }
    }
}

async fn start_turn(host: &Host, text: String) -> Result<()> {
    let message = CustomMessage::new(ENTRY, text)
        .display(true)
        .deliver_as(DeliverAs::FollowUp)
        .trigger_turn(true);
    host.send_message(message).await
}
