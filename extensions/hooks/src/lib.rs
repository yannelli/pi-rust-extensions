mod config;
mod decide;
mod dispatch;
mod events;
mod exec;
mod manage;
mod matcher;
mod model;
mod output;
mod panel;
mod paths;
mod runtime;
mod tools;
mod trust;
mod ui;

use std::future::Future;
use std::process::ExitCode;
use std::sync::Arc;

use pi_ext_sdk::CompletionItem;
use pi_ext_sdk::prelude::*;

pub use paths::Paths;
use runtime::{Runtime, TRUST_ALL_FLAG};

type Handler<F> = fn(Arc<Runtime>, EventCtx) -> F;

fn on<F>(ext: Extension, rt: &Arc<Runtime>, event: &str, handler: Handler<F>) -> Extension
where
    F: Future<Output = Result<EventReply>> + Send + 'static,
{
    let rt = rt.clone();
    ext.on(event, move |ev: EventCtx| handler(rt.clone(), ev))
}

pub fn extension(paths: Paths) -> Extension {
    let rt = Runtime::new(paths);
    let mut ext = Extension::new("hooks").flag(
        FlagSpec::boolean(TRUST_ALL_FLAG)
            .default(false)
            .description("Run all hooks without asking for trust"),
    );
    ext = on(ext, &rt, "session_start", events::session::session_start);
    ext = on(
        ext,
        &rt,
        "session_shutdown",
        events::session::session_shutdown,
    );
    ext = on(
        ext,
        &rt,
        "before_agent_start",
        events::session::before_agent_start,
    );
    ext = on(
        ext,
        &rt,
        "session_before_compact",
        events::session::session_before_compact,
    );
    ext = on(
        ext,
        &rt,
        "session_compact",
        events::session::session_compact,
    );
    ext = on(ext, &rt, "model_select", events::session::model_select);
    ext = on(ext, &rt, "input", events::agent::input);
    ext = on(ext, &rt, "agent_end", events::agent::agent_end);
    ext = on(ext, &rt, "agent_settled", events::agent::agent_settled);
    ext = on(ext, &rt, "tool_call", events::tool::tool_call);
    ext = on(ext, &rt, "tool_result", events::tool::tool_result);
    ext = on(ext, &rt, "turn_end", events::tool::turn_end);
    let command_rt = rt.clone();
    let start_rt = rt.clone();
    ext.command(
        "hooks",
        "List and manage Claude Code, Codex, and pi hooks",
        move |args, ctx: CommandCtx| ui::hooks_command(command_rt.clone(), args, ctx),
    )
    .completions("hooks", |prefix: String, _host: Host| async move {
        Ok(Some(
            ["list"]
                .iter()
                .filter(|c| c.starts_with(&prefix))
                .map(|c| CompletionItem::new(*c))
                .collect(),
        ))
    })
    .on_start(move |host: Host| events::agent::watch_subagents(start_rt.clone(), host))
}

pub fn main() -> ExitCode {
    extension(Paths::from_env()).run()
}
