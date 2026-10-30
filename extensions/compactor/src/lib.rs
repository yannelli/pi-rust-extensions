//! Compactor: lossless zero-LLM or model-written compaction summaries, session recall, context budget.

pub mod bash;
pub mod boundary;
pub mod brief;
pub mod brief_collapse;
pub mod commands;
pub mod compact;
pub mod config;
pub mod cut;
pub mod doctor;
pub mod hooks;
pub mod llm;
pub mod noise;
pub mod plan;
pub mod rank;
pub mod recall;
pub mod source;
pub mod state;
pub mod stats;
pub mod summary;
pub mod text;
pub mod tokens;
pub mod trigger;

use std::process::ExitCode;

use pi_ext_sdk::prelude::*;

use crate::config::Method;
use crate::recall::RecallInput;
use crate::state::Shared;

pub fn main() -> ExitCode {
    extension().run()
}

/// The extension with every tool, command, and event handler registered.
pub fn extension() -> Extension {
    let shared = Shared::default();
    let s = || shared.clone();
    let (s1, s2, s3, s4, s5, s6, s7, s8, s9, s10) =
        (s(), s(), s(), s(), s(), s(), s(), s(), s(), s());
    Extension::new("compactor")
        .tool(commands::recall_spec(), move |input: RecallInput, ctx: ToolCtx| commands::recall_tool(input, ctx, s1.clone()))
        .tool(commands::budget_spec(), |_: Value, ctx: ToolCtx| commands::budget_tool(ctx))
        .command(
            "unipi:compact-vcc",
            "Lossless compaction now — instant structured summary, no model call (keep:N keeps N recent turns)",
            move |args, ctx: CommandCtx| {
                commands::run_compaction(&ctx, &s2, Method::Vcc, &args, "unipi:compact-vcc");
                async { Ok(()) }
            },
        )
        .command(
            "unipi:compact-by-llm",
            "Compact now with a model-written summary (optional text focuses the summary)",
            move |args, ctx: CommandCtx| {
                commands::run_compaction(&ctx, &s3, Method::Llm, &args, "unipi:compact-by-llm");
                async { Ok(()) }
            },
        )
        .command(
            "unipi:session-recall",
            "Recall earlier parts of this session. Plain keywords work best; add scope:all to reach edited or retried turns.",
            commands::session_recall_command,
        )
        .command("unipi:compact-stats", "Show this session's compaction savings", |_, ctx| commands::stats_command(ctx))
        .command("unipi:compact-doctor", "Check compaction settings and leftovers", |_, ctx| commands::doctor_command(ctx))
        .command("unipi:compact-help", "Show compactor help", |_, ctx| commands::help_command(ctx))
        .autocomplete(&["/"], |request, _host| commands::compact_then_suggestions(request))
        .on("session_start", move |_| hooks::on_session_start(s4.clone()))
        .on("context", hooks::on_context)
        .on("input", move |ev| hooks::on_input(ev, s5.clone()))
        .on("session_before_compact", move |ev| compact::on_before_compact(ev, s6.clone()))
        .on("session_compact", move |ev| compact::on_session_compact(ev, s7.clone()))
        .on("turn_end", move |ev| boundary::on_turn_end(ev, s8.clone()))
        .on("agent_end", move |ev| hooks::on_settled(ev, s9.clone()))
        .on("agent_settled", move |ev| hooks::on_settled(ev, s10.clone()))
        .on_start(move |host| state::watch_active_work(host, shared.clone()))
}
