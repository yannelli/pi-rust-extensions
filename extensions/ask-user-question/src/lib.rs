use std::process::ExitCode;

use pi_ext_sdk::Extension;

pub fn main() -> ExitCode {
    Extension::new("ask-user-question").run()
}
