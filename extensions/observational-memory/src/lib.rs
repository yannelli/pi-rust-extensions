use std::process::ExitCode;

use pi_ext_sdk::prelude::*;

pub fn main() -> ExitCode {
    Extension::new("observational-memory").run()
}
