//! The subagents extension under the real pi binary, with `PI_BIN` pointing at a fake child pi.

#![cfg(unix)]

mod common;

use pi_ext_e2e::{Run, TempDir, Turn, cargo_bin, json, pi_bin_or_skip};

#[test]
fn subagent_tool_runs_child_with_session_model() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let fake_dir = TempDir::new().unwrap();
    let fake = common::fake_pi(
        fake_dir.path(),
        &common::scripted_events("main is in src/main.rs"),
        0,
    );
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["subagents"])
        .env("PI_BIN", fake.display().to_string())
        .turns([
            Turn::tool_call("subagent", json!({"agent": "explore", "task": "Find main"})),
            Turn::text("Done."),
        ])
        .prompt("Where is main?")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    assert_eq!(
        output.tool_result_text("subagent").as_deref(),
        Some("main is in src/main.rs"),
        "{}",
        output.stderr
    );
    assert!(!output.tool_updates("subagent").is_empty());
    assert!(output.tool_names(0).contains(&"subagent".to_string()));
    assert!(output.system_prompt(0).contains("## Subagents"));
    let args = std::fs::read_to_string(fake_dir.path().join("args")).unwrap();
    let args: Vec<&str> = args.lines().collect();
    let model = args
        .iter()
        .position(|a| *a == "--model")
        .map(|i| args[i + 1]);
    assert_eq!(model, Some("mock/mock-model"));
    let env = std::fs::read_to_string(fake_dir.path().join("env")).unwrap();
    assert!(env.contains("PI_TODO_SCOPE="), "{env}");
    assert_eq!(output.final_text(), "Done.");
}
