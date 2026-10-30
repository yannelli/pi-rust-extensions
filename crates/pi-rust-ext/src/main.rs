//! Multi-call binary: `pi-rust-ext <extension> [args...]` runs one marketplace extension.

use std::process::ExitCode;

type Extension = (&'static str, fn() -> ExitCode);

const EXTENSIONS: &[Extension] = &[
    ("lens", pi_ext_lens::main),
    ("typesafe", pi_ext_typesafe::main),
    ("background-tasks", pi_ext_background_tasks::main),
    ("goal", pi_ext_goal::main),
    ("rewind", pi_ext_rewind::main),
    ("ssh", pi_ext_ssh::main),
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
