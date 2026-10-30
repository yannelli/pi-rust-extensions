//! Multi-call binary: `pi-rust-ext <extension> [args...]` runs one marketplace extension.

use std::process::ExitCode;

type Extension = (&'static str, fn() -> ExitCode);

const EXTENSIONS: &[Extension] = &[
    ("compactor", pi_ext_compactor::main),
    ("observational-memory", pi_ext_observational_memory::main),
    ("subscription-router", pi_ext_subscription_router::main),
    ("lens", pi_ext_lens::main),
    ("typesafe", pi_ext_typesafe::main),
    ("background-tasks", pi_ext_background_tasks::main),
    ("goal", pi_ext_goal::main),
    ("rewind", pi_ext_rewind::main),
    ("ssh", pi_ext_ssh::main),
    ("iterm2", pi_ext_iterm2::main),
    ("ollama", pi_ext_ollama::main),
    ("statusline", pi_ext_statusline::main),
    ("ask-user-question", pi_ext_ask_user_question::main),
    ("subagents", pi_ext_subagents::main),
    ("todo", pi_ext_todo::main),
    ("hooks", pi_ext_hooks::main),
    ("permissions", pi_ext_permissions::main),
    ("claude-marketplace", pi_ext_claude_marketplace::main),
    ("context-include", pi_ext_context_include::main),
    ("web-search", pi_ext_web_search::main),
    ("chrome", pi_ext_chrome::main),
    ("computer-use", pi_ext_computer_use::main),
];

fn main() -> ExitCode {
    let name = std::env::args().nth(1).unwrap_or_default();
    match EXTENSIONS.iter().find(|(id, _)| *id == name) {
        Some((_, run)) => run(),
        None => {
            let names: Vec<&str> = EXTENSIONS.iter().map(|(id, _)| *id).collect();
            eprintln!(
                "usage: pi-rust-ext <extension>\nextensions: {}",
                names.join(", ")
            );
            ExitCode::from(2)
        }
    }
}
