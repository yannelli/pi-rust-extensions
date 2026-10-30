//! Observational memory: observer, reflector and dropper workers record session memory,
//! the memory becomes the compaction summary, and `recall` returns its source evidence.

use std::process::ExitCode;
use std::sync::Arc;

use pi_ext_sdk::prelude::*;
use tokio::sync::OnceCell;

pub mod agents;
pub mod clipboard;
pub mod commands;
pub mod config;
pub mod debug_log;
pub mod hooks;
pub mod host_glue;
pub mod ledger;
pub mod pipeline;
pub mod prompts;
pub mod recall_tool;
pub mod runtime;
pub mod serialize;
pub mod stages;
pub mod tokens;

use config::Config;
use runtime::Runtime;

/// Shared by all handlers; the config loads on first use.
#[derive(Default)]
pub struct State {
    pub runtime: Runtime,
    config: OnceCell<Config>,
}

impl State {
    pub async fn config(&self, host: &Host, snapshot: &Snapshot) -> Config {
        self.config
            .get_or_init(|| host_glue::load_config(host, snapshot, &self.runtime))
            .await
            .clone()
    }
}

async fn status_command(state: Arc<State>, ctx: CommandCtx) -> Result<()> {
    let host = ctx.host();
    let config = state.config(host, ctx.snapshot()).await;
    let entries = host.branch(None).await?;
    let window = ctx.snapshot().model.as_ref().and_then(|m| m.context_window);
    let text = commands::status_text(&entries, &config, &state.runtime, window);
    host.notify_info(text).await
}

async fn view_command(args: String, ctx: CommandCtx) -> Result<()> {
    let host = ctx.host();
    let entries = host.branch(None).await?;
    let Some(output) = commands::view_text(&entries, &args) else {
        return host.notify_info("Usage: /om:view [full]").await;
    };
    let footer = if clipboard::copy_text(host, &output).await {
        "Copied /om:view output to clipboard."
    } else {
        "Warning: failed to copy /om:view output to clipboard."
    };
    host.notify_info(format!("{output}\n\n{footer}")).await
}

pub fn extension() -> Extension {
    let state = Arc::new(State::default());
    let (launch, settled, compact, status) = (state.clone(), state.clone(), state.clone(), state);
    let launch_turn = launch.clone();
    Extension::new("observational-memory")
        .tool(recall_tool::spec(), recall_tool::execute)
        .command("om:status", "Show observational memory status", move |_, ctx| {
            status_command(status.clone(), ctx)
        })
        .command(
            "om:view",
            "Print and copy observational memory content (visible by default, full for recorded memory)",
            view_command,
        )
        .on("agent_start", move |ev: EventCtx| {
            hooks::launch_consolidation(launch.clone(), &ev);
            async { Ok(()) }
        })
        .on("turn_end", move |ev: EventCtx| {
            hooks::launch_consolidation(launch_turn.clone(), &ev);
            async { Ok(()) }
        })
        .on("agent_settled", move |ev: EventCtx| {
            hooks::launch_compaction(settled.clone(), &ev);
            async { Ok(()) }
        })
        .on("session_before_compact", move |ev: EventCtx| {
            hooks::before_compact(compact.clone(), ev)
        })
}

pub fn main() -> ExitCode {
    extension().run()
}
