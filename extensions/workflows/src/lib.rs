//! Workflows: fixed and dynamic multi-agent pipelines. Each step runs as its own pi agent with
//! its own model, thinking level, tools, skills, MCP servers, subagent rules, and environment.

use std::process::ExitCode;

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::{CompletionItem, MessageRender};

pub mod app;
pub mod child;
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

/// How many runs `/workflows` and `status` keep from earlier pi sessions.
const KEEP_RUNS: usize = 30;

pub fn extension() -> Extension {
    if std::env::var_os(policy::RUN_ENV).is_some() {
        return policy::child_extension();
    }
    let app = App::default();
    let (tool_app, command_app, start_app, stop_app) = (app.clone(), app.clone(), app.clone(), app);
    Extension::new("workflows")
        .tool(tool::spec(), move |params: tool::Params, ctx: ToolCtx| {
            tool::execute(tool_app.clone(), params, ctx)
        })
        .command(
            "workflows",
            "Create, edit, run, and watch workflows: /workflows [new | runs | edit <name> | run <name>]",
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
            Ok(Some(items.into_iter().filter(|i| i.starts_with(prefix)).map(CompletionItem::new).collect()))
        })
        .message_renderer(runner::MESSAGE_TYPE, |req: MessageRender, host: Host| async move {
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
        .on_start(move |host: Host| {
            let app = start_app.clone();
            async move {
                let root = app.runs_root(&host);
                runs::prune(&root);
                app.runs.load(&root, KEEP_RUNS);
                Ok(())
            }
        })
        .on_shutdown(move |_host: Host| {
            let app = stop_app.clone();
            async move {
                app.shutdown().await;
                Ok(())
            }
        })
}

pub fn main() -> ExitCode {
    extension().run()
}
