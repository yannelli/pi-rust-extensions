//! Per-turn shadow git checkpoints of the workspace and `/rewind`.

use std::process::ExitCode;
use std::sync::Arc;

use pi_ext_sdk::prelude::*;

pub mod checkpoint;
pub mod config;
mod excludes;
pub mod git;
mod gitcmd;
pub mod lifecycle;
pub mod lock;
mod nested;
pub mod rewind;
pub mod state;
#[doc(hidden)]
pub mod testdir;
pub mod tree;

use state::State;

pub fn extension() -> Extension {
    let state = Arc::new(State::default());
    let on_start = state.clone();
    let on_message = state.clone();
    let on_agent_end = state.clone();
    let on_fork = state.clone();
    let on_shutdown = state.clone();
    let on_before_tree = state.clone();
    let on_tree = state.clone();
    let on_input = state.clone();
    let on_compact = state.clone();
    Extension::new("rewind")
        .command(
            "rewind",
            "Restore code, conversation, or both to an earlier turn",
            move |_args, ctx| rewind::run(state.clone(), ctx),
        )
        .on("session_start", move |ev| {
            lifecycle::session_start(on_start.clone(), ev)
        })
        .on("message_start", move |ev| {
            lifecycle::message_start(on_message.clone(), ev)
        })
        .on("agent_end", move |ev: EventCtx| {
            let state = on_agent_end.clone();
            async move {
                lifecycle::finish(&state, ev.host()).await;
                Ok(())
            }
        })
        .on("session_before_fork", move |ev| {
            lifecycle::before_fork(on_fork.clone(), ev)
        })
        .on("session_shutdown", move |ev: EventCtx| {
            let state = on_shutdown.clone();
            async move {
                lifecycle::finish(&state, ev.host()).await;
                Ok(())
            }
        })
        .on("session_before_tree", move |ev| {
            tree::before_tree(on_before_tree.clone(), ev)
        })
        .on("session_tree", move |ev| {
            tree::after_tree(on_tree.clone(), ev)
        })
        .on("input", move |ev| lifecycle::input(on_input.clone(), ev))
        .on("session_compact", move |ev| {
            lifecycle::session_compact(on_compact.clone(), ev)
        })
}

pub fn main() -> ExitCode {
    extension().run()
}
