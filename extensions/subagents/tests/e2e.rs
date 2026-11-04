//! The subagents extension under the real pi binary, with `PI_BIN` pointing at a fake child pi.

#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;

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

#[test]
fn real_child_writes_its_session_file() {
    let Some(pi) = pi_bin_or_skip() else {
        return;
    };
    let keep = TempDir::new().unwrap();
    let copy = keep.path().join("session.jsonl");
    let wrapper = keep.path().join("child-pi");
    let script = format!(
        "#!/bin/sh\n'{}' \"$@\"\ncode=$?\nprev=\nfor a in \"$@\"; do [ \"$prev\" = --session ] && cp \"$a\" '{}'; prev=$a; done\nexit $code\n",
        pi.display(),
        copy.display()
    );
    std::fs::write(&wrapper, script).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["subagents"])
        .env("PI_BIN", wrapper.display().to_string())
        .turns([
            Turn::tool_call("subagent", json!({"agent": "explore", "task": "Find main"})),
            Turn::text("main is in src/main.rs"),
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
    let session = std::fs::read_to_string(&copy).unwrap();
    let entries: Vec<serde_json::Value> = session
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(entries[0]["type"], "session");
    let message_of = |role: &str| {
        entries
            .iter()
            .find(|e| e["message"]["role"] == role)
            .map(|e| e["message"]["content"][0]["text"].clone())
    };
    assert_eq!(message_of("user"), Some(json!("Find main")), "{session}");
    assert_eq!(
        message_of("assistant"),
        Some(json!("main is in src/main.rs")),
        "{session}"
    );
}
