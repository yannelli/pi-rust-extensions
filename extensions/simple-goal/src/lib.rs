//! One objective that keeps the agent working until an evaluator agrees it is done.

mod actions;
mod config;
mod evaluator;
mod settle;
mod state;

#[cfg(test)]
mod tests;

use std::process::ExitCode;

use pi_ext_sdk::prelude::*;

pub use state::ENTRY;

pub fn main() -> ExitCode {
    extension().run()
}

pub fn extension() -> Extension {
    let app = state::App::default();
    let tool_app = app.clone();
    let command_app = app.clone();
    let mut ext = Extension::new("simple-goal")
        .tool(actions::tool_spec(), move |args, ctx| {
            actions::run_tool(tool_app.clone(), args, ctx)
        })
        .command(actions::COMMAND, actions::COMMAND_HELP, move |args, ctx| {
            actions::run_command(command_app.clone(), args, ctx)
        });
    for name in ["session_start", "session_tree"] {
        let app = app.clone();
        ext = ext.on(name, move |ev: EventCtx| {
            let app = app.clone();
            async move { app.restore(ev.host()).await }
        });
    }
    let calls_app = app.clone();
    ext = ext.on("tool_call", move |ev: EventCtx| {
        let name = ev.get("toolName").and_then(Value::as_str);
        if name != Some(actions::TOOL) {
            calls_app.lock().tool_calls += 1;
        }
        async { Ok(()) }
    });
    ext.on("agent_settled", move |ev: EventCtx| {
        let app = app.clone();
        async move {
            let aborted = ev.get("aborted").and_then(Value::as_bool).unwrap_or(false);
            settle::on_settled(&app, ev.host(), aborted).await
        }
    })
}
