//! Multi-call binary: `pi-rust-ext <extension> [args...]` runs one marketplace extension.

use std::process::ExitCode;

type Extension = (&'static str, fn() -> ExitCode);

const EXTENSIONS: &[Extension] = &[
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
