//! iTerm2 integration for pi-rust: tab color by agent state, session badge, status bar user
//! variables, attention on long runs, and the cwd. `/terminal-setup` turns it on.

pub mod features;
pub mod reporter;
pub mod sequences;
pub mod setup;

use std::process::ExitCode;
use std::sync::Arc;

use pi_ext_sdk::prelude::*;

use reporter::{Reporter, Terminal};
use sequences::{EnvLookup, ProcessEnv, is_iterm2};

/// Events whose ctx snapshot can carry a new run state, session name, or model.
const REPORT_EVENTS: &[&str] = &[
    "session_start",
    "session_tree",
    "session_compact",
    "agent_start",
    "agent_end",
    "turn_start",
    "turn_end",
    "message_start",
    "message_end",
    "tool_execution_start",
    "tool_execution_end",
    "model_select",
];

pub fn extension(terminal: Terminal) -> Extension {
    let reporter = Arc::new(Reporter::new(terminal));
    let mut ext = Extension::new("iterm2");
    for event in REPORT_EVENTS {
        let reporter = reporter.clone();
        ext = ext.on(event, move |ev: EventCtx| {
            let reporter = reporter.clone();
            async move {
                match ev.name() {
                    "agent_start" => reporter.run_started(),
                    "agent_end" => reporter.run_finished(ev.host()).await,
                    _ => {}
                }
                reporter.report(ev.host(), ev.snapshot()).await;
                Ok(())
            }
        });
    }
    let setup = reporter.clone();
    ext.command(
        "terminal-setup",
        "Set up iTerm2: the pi integration, clipboard access, and Option as Alt",
        move |_args, ctx: CommandCtx| setup::terminal_setup(setup.clone(), ctx),
    )
    .on_shutdown(move |host: Host| {
        let reporter = reporter.clone();
        async move {
            reporter.reset(&host).await;
            Ok(())
        }
    })
}

pub fn main() -> ExitCode {
    let terminal = Terminal {
        iterm2: is_iterm2(&ProcessEnv),
        tmux: ProcessEnv.non_empty("TMUX").is_some(),
    };
    extension(terminal).run()
}
