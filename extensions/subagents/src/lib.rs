//! Subagents: run tasks in child pi processes with explore, build, advisor, and custom agents.

use std::process::ExitCode;

use pi_ext_sdk::prelude::*;

pub mod actions;
pub mod active_work;
pub mod agent;
pub mod app;
pub mod command;
pub mod config;
pub mod context;
pub mod defaults;
pub mod exec;
pub mod frontmatter;
pub mod jobs;
pub mod messaging;
pub mod registry;
pub mod render;
pub mod runs;
pub mod runs_panel;
pub mod store;
pub mod stream;
pub mod tool;
pub mod ui;

use app::App;

pub fn extension() -> Extension {
    let app = App::default();
    let tool_app = app.clone();
    let command_app = app.clone();
    let complete_app = app.clone();
    let prompt_app = app.clone();
    let discover_app = app.clone();
    let surface_app = app.clone();
    let sync_app = app.clone();
    let messaging_app = app.clone();
    let shutdown_app = app;
    Extension::new("subagents")
        .tool(tool::spec(), move |params: tool::Params, ctx: ToolCtx| {
            tool::execute(tool_app.clone(), params, ctx)
        })
        .command(
            "agents",
            "Manage subagents in one panel: edit every field, new, copy, rename, move, delete or reset, and runs (`/agents runs`, `/agents list`, `/agents <name>`)",
            move |args: String, ctx: CommandCtx| agents_command(command_app.clone(), args, ctx),
        )
        .completions("agents", move |prefix: String, host: Host| {
            let app = complete_app.clone();
            async move {
                let names = app.agents(&host).into_iter().map(|a| a.name);
                let words = ["runs".to_owned(), "list".to_owned()].into_iter().chain(names);
                let items = words
                    .filter(|item| item.starts_with(prefix.trim()))
                    .map(pi_ext_sdk::CompletionItem::new)
                    .collect();
                Ok(Some(items))
            }
        })
        .on("before_agent_start", move |ev: EventCtx| {
            let app = prompt_app.clone();
            async move { Ok(advertise(&app, &ev)) }
        })
        .on("resources_discover", move |ev: EventCtx| {
            let app = discover_app.clone();
            async move { Ok(context::discover(&app, ev.host())) }
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
        .on_start(move |host: Host| active_work::serve(sync_app.clone(), host))
        .on_start(move |host: Host| messaging::serve(messaging_app.clone(), host))
        .on_shutdown(move |host: Host| {
            let app = shutdown_app.clone();
            async move {
                messaging::close(&host);
                app.runs.cancel_all();
                let deadline = tokio::time::Instant::now() + exec::SHUTDOWN_WAIT;
                while app.runs.any_running() && tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
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
    if sub == "list" || !host.has_ui() {
        let text = if sub == "runs" {
            let runs = app.runs.all();
            if runs.is_empty() {
                "No subagent runs.".to_owned()
            } else {
                render::progress_text(&runs)
            }
        } else {
            let listing = actions::list_agents(&app.agents(&host));
            listing.content[0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        };
        return host.notify_info(text).await;
    }
    let start = match sub {
        "runs" => ui::Start::Runs,
        "" => ui::Start::Agents(None),
        name => ui::Start::Agents(Some(registry::sanitize_name(name))),
    };
    ui::open(app, host, start).await
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
    let agents = app.agents(host);
    let offered = app.offered.names().filter(|_| context::host_supports(host));
    EventReply::section(
        "subagents",
        context::prompt_section(&agents, offered.as_ref()),
    )
}
