//! Multi-call binary: `pi-rust-ext <extension> [args...]` runs one marketplace extension.

use std::process::ExitCode;

type Extension = (&'static str, fn() -> ExitCode);

const EXTENSIONS: &[Extension] = &[
    ("ask-user-question", pi_ext_ask_user_question::main),
    ("subagents", pi_ext_subagents::main),
    ("todo", pi_ext_todo::main),
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
