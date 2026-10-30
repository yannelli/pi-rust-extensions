mod config;
mod detection;
mod diagnostics;
mod events;
mod lsp;
mod pipeline;
mod state;
mod tools;
mod tui;

use std::process::ExitCode;
use std::sync::Arc;

use pi_ext_sdk::prelude::*;
use tokio::sync::Mutex;

use events::{handle, refresh};
use state::State;
use tools::get_diagnostics;

pub fn main() -> ExitCode {
    extension().run()
}

pub fn extension() -> Extension {
    let state = Arc::new(Mutex::new(State::default()));
    let mut extension = Extension::new("lens");
    for name in ["no-lens", "no-lens-context", "no-read-guard"] {
        extension = extension.flag(FlagSpec::boolean(name).default(false));
    }
    for event in [
        "session_start",
        "session_tree",
        "session_compact",
        "tool_call",
        "tool_result",
        "context",
    ] {
        let state = state.clone();
        extension = extension.on(event, move |ctx: EventCtx| {
            let state = state.clone();
            async move { handle(ctx, state).await }
        });
    }
    let command_state = state.clone();
    extension = extension.command(
        "lens",
        "Manage formatters, linters, type checkers and LSP servers",
        move |_, ctx: CommandCtx| {
            let state = command_state.clone();
            async move {
                refresh(ctx.host(), &state).await?;
                tui::manage(ctx.host(), state).await
            }
        },
    );
    let command_state = state.clone();
    extension = extension.command(
        "lens-allow-edit",
        "Allow one edit without a prior read",
        move |args, ctx: CommandCtx| {
            let state = command_state.clone();
            async move {
                if args.trim().is_empty() {
                    return Err(Error::msg("Usage: /lens-allow-edit <path>"));
                }
                let path = diagnostics::normalize_path(&ctx.host().cwd(), args.trim());
                state.lock().await.guard.allowed.insert(path);
                Ok(())
            }
        },
    );
    for (name, context) in [("lens-toggle", false), ("lens-context-toggle", true)] {
        let state = state.clone();
        extension = extension.command(
            name,
            "Toggle Lens for this session",
            move |_, _ctx: CommandCtx| {
                let state = state.clone();
                async move {
                    let mut state = state.lock().await;
                    if context {
                        state.config.context = !state.config.context;
                    } else {
                        state.config.enabled = !state.config.enabled;
                    }
                    Ok(())
                }
            },
        );
    }
    let tool_state = state.clone();
    extension = extension.tool(
        ToolSpec::new("effective_config", "Show the effective Lens configuration")
            .parameters(json!({"type":"object","properties":{"file":{"type":"string"}}})),
        move |_args: Value, _ctx: ToolCtx| {
            let state = tool_state.clone();
            async move { Ok(serde_json::to_string_pretty(&state.lock().await.config)?) }
        },
    );
    extension.tool(ToolSpec::new("lens_diagnostics", "Read session diagnostics or request current LSP diagnostics").parameters(json!({
        "type":"object", "properties":{
            "source":{"enum":["session","lsp"]}, "scope":{"enum":["paths","workspace"]},
            "paths":{"type":"array","items":{"type":"string"}}, "severity":{"enum":["error","warning","information","hint"]}, "mode":{"enum":["full"]}
        }
    })), move |args: Value, ctx: ToolCtx| {
        let state = state.clone();
        async move { get_diagnostics(args, ctx, state).await }
    })
}

#[cfg(test)]
mod tests;
