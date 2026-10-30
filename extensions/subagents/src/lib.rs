//! Subagents: run tasks in child pi processes with explore, build, advisor, and custom agents.

use std::process::ExitCode;

use pi_ext_sdk::prelude::*;

pub mod actions;
pub mod agent;
mod agents_edit;
mod agents_menu;
pub mod app;
pub mod command;
pub mod config;
pub mod defaults;
pub mod exec;
pub mod frontmatter;
pub mod jobs;
pub mod registry;
pub mod render;
pub mod runs;
pub mod runs_panel;
pub mod stream;
pub mod tool;

use app::App;

pub fn extension() -> Extension {
    let app = App::default();
    let tool_app = app.clone();
    let command_app = app.clone();
    let prompt_app = app.clone();
    let surface_app = app.clone();
    let shutdown_app = app;
    Extension::new("subagents")
        .tool(tool::spec(), move |params: tool::Params, ctx: ToolCtx| {
            tool::execute(tool_app.clone(), params, ctx)
        })
        .command(
            "agents",
            "Manage subagents: list, create, edit, model, tools, reset, delete; `/agents runs` for runs",
            move |args: String, ctx: CommandCtx| agents_command(command_app.clone(), args, ctx),
        )
        .completions("agents", |prefix: String, _host: Host| async move {
            let items = ["runs", "list"]
                .into_iter()
                .filter(|item| item.starts_with(prefix.trim()))
                .map(pi_ext_sdk::CompletionItem::new)
                .collect();
            Ok(Some(items))
        })
        .on("before_agent_start", move |ev: EventCtx| {
            let app = prompt_app.clone();
            async move { Ok(advertise(&app, &ev)) }
        })
        .on_surface(move |event: SurfaceEvent, host: Host| {
            let app = surface_app.clone();
            async move {
                if matches!(event, SurfaceEvent::Resize { .. }) && event.widget_key() == Some(app::WIDGET) {
                    app.draw_widget(&host).await;
                }
                Ok(())
            }
        })
        .on_shutdown(move |_host: Host| {
            let app = shutdown_app.clone();
            async move {
                app.runs.cancel_all();
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                Ok(())
            }
        })
}

pub fn main() -> ExitCode {
    extension().run()
}

async fn agents_command(app: App, args: String, ctx: CommandCtx) -> Result<()> {
    let host = ctx.host().clone();
    let sub = args.trim();
    if sub == "runs" && host.has_ui() {
        return runs_panel::open(app, host).await;
    }
    if sub == "list" || sub == "runs" || !host.has_ui() {
        let text = if sub == "runs" {
            let runs = app.runs.all();
            if runs.is_empty() {
                "No subagent runs.".to_owned()
            } else {
                render::progress_text(&runs)
            }
        } else {
            render::prompt_section(&app.agents(&host))
        };
        return host.notify_info(text).await;
    }
    agents_menu::open(app, host).await
}

/// Adds the `subagents` system prompt section when the `subagent` tool is active.
fn advertise(app: &App, ev: &EventCtx) -> EventReply {
    let host = ev.host();
    if !app.config(host).advertise {
        return EventReply::none();
    }
    let selected = ev
        .event()
        .pointer("/systemPromptOptions/selectedTools")
        .and_then(Value::as_array);
    let active = selected.is_none_or(|tools| tools.iter().any(|t| t == "subagent"));
    if !active {
        return EventReply::none().with_section("subagents", None);
    }
    EventReply::section("subagents", render::prompt_section(&app.agents(host)))
}
