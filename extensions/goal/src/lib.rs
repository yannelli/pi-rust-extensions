//! Persistent goals for pi: drafting, a task tree, autonomous continuation, and a completion auditor.

mod app;
mod auditor;
mod commands;
mod config;
mod draft;
mod draft_tools;
mod events;
mod format;
mod lifecycle;
mod prompts;
mod report;
mod scheduler;
mod settings_ui;
mod specs;
mod state;
mod store;
mod tasks;
mod tools;
mod update;
mod view;
mod widget;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_io;

use std::process::ExitCode;

use pi_ext_sdk::prelude::*;

pub use app::{AUDIT_MESSAGE, DRAFT_ENTRY, EVENT_MESSAGE, FOCUS_ENTRY};

pub const WAKE_CHANNEL: &str = "pi-goal:wake";

pub fn main() -> ExitCode {
    extension().run()
}

macro_rules! tool {
    ($ext:expr, $app:expr, $spec:expr, $handler:path) => {{
        let app = $app.clone();
        $ext.tool($spec, move |args, ctx| $handler(app.clone(), args, ctx))
    }};
}

pub fn extension() -> Extension {
    let app = app::App::default();
    let [create, get, update, set_tasks, update_task] = specs::goal_tools();
    let [question, questionnaire, propose] = specs::draft_tools();
    let mut ext = Extension::new("goal");
    ext = tool!(ext, app, create, tools::create_goal);
    let get_app = app.clone();
    ext = ext.tool(get, move |args: tools::GetArgs, _ctx: ToolCtx| {
        let result = tools::get_goal(&get_app, args);
        async move { Ok(result) }
    });
    ext = tool!(ext, app, update, tools::update_goal);
    ext = tool!(ext, app, set_tasks, tools::set_goal_tasks);
    ext = tool!(ext, app, update_task, tools::update_goal_task);
    ext = tool!(ext, app, question, draft_tools::goal_question);
    ext = tool!(ext, app, questionnaire, draft_tools::goal_questionnaire);
    ext = tool!(ext, app, propose, draft_tools::propose_goal_draft);
    for (name, description) in commands::COMMANDS {
        let run_app = app.clone();
        ext = ext.command(name, description, move |args, ctx| {
            let app = run_app.clone();
            async move { commands::run(app, name, args, ctx).await }
        });
        let complete_app = app.clone();
        if commands::completions(&complete_app, name, "").is_some() {
            ext = ext.completions(name, move |prefix, _host| {
                let items = commands::completions(&complete_app, name, &prefix);
                async move { Ok(items) }
            });
        }
    }
    let shortcut_app = app.clone();
    ext = ext.shortcut(
        "ctrl+shift+t",
        "Expand or collapse the goal widget",
        move |ctx| {
            let app = shortcut_app.clone();
            async move {
                {
                    let mut inner = app.lock();
                    inner.expanded = !inner.expanded;
                }
                widget::refresh(&app, ctx.host()).await
            }
        },
    );
    events(ext, app)
}

fn events(mut ext: Extension, app: app::App) -> Extension {
    for name in ["session_start", "session_tree", "session_compact"] {
        let app = app.clone();
        ext = ext.on(name, move |ev: EventCtx| {
            let app = app.clone();
            async move { events::on_session(&app, ev.host()).await }
        });
    }
    let a = app.clone();
    ext = ext.on("before_agent_start", move |ev: EventCtx| {
        let app = a.clone();
        async move { events::before_agent_start(&app, &ev).await }
    });
    let a = app.clone();
    ext = ext.on("tool_call", move |ev: EventCtx| {
        let reply = events::tool_call(&a, &ev);
        async move { Ok(reply) }
    });
    let a = app.clone();
    ext = ext.on("message_end", move |ev: EventCtx| {
        let app = a.clone();
        async move { events::message_end(&app, &ev).await }
    });
    let a = app.clone();
    ext = ext.on("agent_start", move |_ev: EventCtx| {
        events::agent_start(&a);
        async { Ok(()) }
    });
    let a = app.clone();
    ext = ext.on("agent_end", move |_ev: EventCtx| {
        events::agent_end(&a);
        async { Ok(()) }
    });
    let a = app.clone();
    ext = ext.on("agent_settled", move |ev: EventCtx| {
        let app = a.clone();
        async move {
            let aborted = ev.get("aborted").and_then(Value::as_bool).unwrap_or(false);
            scheduler::on_settled(&app, ev.host(), aborted).await
        }
    });
    ext.on_start(move |host: Host| {
        let app = app.clone();
        async move {
            let mut wake = host.subscribe(WAKE_CHANNEL).await?;
            while let Some(data) = wake.recv().await {
                let goal_id = data
                    .get("goalId")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let focused = app.lock().focus.clone();
                if let Some(id) = goal_id.or(focused) {
                    scheduler::wake(&app, &host, &id, None).await;
                }
            }
            Ok(())
        }
    })
}
