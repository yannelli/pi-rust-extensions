use pi_ext_e2e::{Run, Turn, cargo_bin, json, pi_bin_or_skip};

#[test]
fn pre_tool_use_hook_denies_bash_in_pi() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let claude = tempfile::tempdir().unwrap();
    let settings = json!({ "hooks": { "PreToolUse": [{
        "matcher": "Bash",
        "hooks": [{ "type": "command", "command": "cat > /dev/null; echo 'denied by hook' >&2; exit 2" }]
    }] } });
    std::fs::write(claude.path().join("settings.json"), settings.to_string()).unwrap();
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["hooks"])
        .env("CLAUDE_CONFIG_DIR", claude.path().display().to_string())
        .pi_args(["--hooks-trust-all"])
        .turns([
            Turn::tool_call("bash", json!({ "command": "echo hi" })),
            Turn::text("done"),
        ])
        .prompt("Run echo hi")
        .run()
        .unwrap();
    let text = output.tool_result_text("bash").unwrap_or_default();
    assert!(
        text.contains("denied by hook"),
        "tool result: {text:?}\nstderr: {}",
        output.stderr
    );
    assert!(!text.contains("\nhi"), "bash ran: {text:?}");
}
