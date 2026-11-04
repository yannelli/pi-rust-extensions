//! Workflows: fixed and dynamic multi-agent pipelines. Each step runs as its own pi agent with
//! its own model, thinking level, tools, skills, MCP servers, subagent rules, and environment.
//! A runner process runs each workflow, so background runs keep going after pi quits.

use std::process::ExitCode;

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::{CompletionItem, MessageRender};

pub mod app;
pub mod child;
pub mod control;
pub mod launch;
pub mod model;
pub mod planner;
pub mod policy;
pub mod runner;
pub mod runs;
pub mod store;
pub mod tool;
pub mod ui;

use app::App;

pub fn extension() -> Extension {
    if std::env::var_os(policy::RUN_ENV).is_some() {
        return policy::child_extension();
    }
    let app = App::default();
    let (tool_app, command_app, start_app) = (app.clone(), app.clone(), app.clone());
    let (event_app, stop_app) = (app.clone(), app);
    Extension::new("workflows")
        .tool(tool::spec(), move |params: tool::Params, ctx: ToolCtx| {
            tool::execute(tool_app.clone(), params, ctx)
        })
        .command(
            "workflows",
            "Create, edit, run, and watch workflows: /workflows [new | runs | edit <name> | run <name> | approve <run id>]",
            move |args: String, ctx: CommandCtx| ui::command(command_app.clone(), args, ctx),
        )
        .completions("workflows", |prefix: String, host: Host| async move {
            let dirs = store::Dirs::new(&host.agent_dir(), &host.project_dir(), app::trusted(&host));
            let mut items: Vec<String> = vec!["new".into(), "runs".into()];
            for stored in dirs.load().0 {
                items.push(format!("run {}", stored.workflow.name));
                items.push(format!("edit {}", stored.workflow.name));
            }
            let prefix = prefix.trim_start();
            let items = items.into_iter().filter(|i| i.starts_with(prefix));
            Ok(Some(items.map(CompletionItem::new).collect()))
        })
        .message_renderer(app::MESSAGE_TYPE, |req: MessageRender, host: Host| async move {
            let paint = ui::theme::Paint::load(&host).await;
            Ok(ui::widget::report_card(&req.message["details"], &paint, req.width))
        })
        .on("resources_discover", |ev: EventCtx| async move {
            Ok(match store::install_skills(&ev.host().agent_dir()) {
                Ok(dir) => EventReply::result(json!({ "skillPaths": [dir] })),
                Err(err) => {
                    pi_ext_sdk::warn!("cannot write the workflow skills: {err}");
                    EventReply::none()
                }
            })
        })
        .on("session_shutdown", move |ev: EventCtx| {
            let reason = ev.get("reason").and_then(Value::as_str).map(str::to_owned);
            event_app.on_shutdown_event(reason);
            async { Ok(()) }
        })
        .on_start(move |host: Host| {
            let app = start_app.clone();
            async move {
                runs::prune(&app.runs_root(&host));
                app.watch_runs(host).await;
                Ok(())
            }
        })
        .on_shutdown(move |host: Host| {
            stop_app.shutdown(&host);
            async { Ok(()) }
        })
}

pub fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.get(2).map(String::as_str) == Some(runner::FLAG) {
        return runner::main(args.get(3).map(String::as_str));
    }
    extension().run()
}
