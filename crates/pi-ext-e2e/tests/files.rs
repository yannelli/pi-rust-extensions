//! `Run::file` places context files in the run's cwd and agent dir before pi starts.

use pi_ext_e2e::{Run, Turn, cargo_example, pi_bin_or_skip};

#[test]
fn run_files_reach_the_system_prompt() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let hello = cargo_example("pi-ext-sdk", "hello");
    let output = Run::new(hello)
        .args(["hello"])
        .file("agent/AGENTS.md", "Global rule from the agent dir.")
        .file("work/AGENTS.md", "Project rule from the cwd.")
        .turns([Turn::text("ok")])
        .prompt("hi")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    let system = output.system_prompt(0);
    assert!(
        system.contains("Global rule from the agent dir."),
        "{system}"
    );
    assert!(system.contains("Project rule from the cwd."), "{system}");
}
