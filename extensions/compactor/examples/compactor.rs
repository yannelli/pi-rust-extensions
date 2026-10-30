//! Standalone compactor binary for end-to-end tests, without the other dispatcher extensions.

use std::process::ExitCode;

fn main() -> ExitCode {
    pi_ext_compactor::main()
}
