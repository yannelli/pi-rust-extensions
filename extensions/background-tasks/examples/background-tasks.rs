//! Runs the extension on stdin and stdout, for the e2e test before the dispatcher lists it.

fn main() -> std::process::ExitCode {
    pi_ext_background_tasks::main()
}
