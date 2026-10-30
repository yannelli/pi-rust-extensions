//! Multi-call binary: `pi-rust-ext <extension> [args...]` runs one marketplace extension.

use std::process::ExitCode;

type Extension = (&'static str, fn() -> ExitCode);

const EXTENSIONS: &[Extension] = &[
    ("compactor", pi_ext_compactor::main),
    ("observational-memory", pi_ext_observational_memory::main),
    ("subscription-router", pi_ext_subscription_router::main),
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
