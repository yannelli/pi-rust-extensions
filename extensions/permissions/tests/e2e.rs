use pi_ext_e2e::{Run, Turn, cargo_bin, json, pi_bin_or_skip};

#[test]
fn deny_rule_blocks_bash_and_the_model_sees_the_reason() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let claude = tempfile::tempdir().unwrap();
    let settings = json!({ "permissions": { "deny": ["Bash(echo *)"] } });
    std::fs::write(claude.path().join("settings.json"), settings.to_string()).unwrap();
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["permissions"])
        .env("CLAUDE_CONFIG_DIR", claude.path().display().to_string())
        .file(
            "agent/ext/permissions.json",
            r#"{"deny": ["Bash(touch *)"]}"#,
        )
        .turns([
            Turn::tool_call("bash", json!({ "command": "echo hi && echo again" })),
            Turn::tool_call("bash", json!({ "command": "touch made.txt" })),
            Turn::text("done"),
        ])
        .prompt("Run echo hi")
        .run()
        .unwrap();
    let results: Vec<String> = output
        .tool_results("bash")
        .iter()
        .map(|e| e["result"]["content"].to_string())
        .collect();
    assert_eq!(results.len(), 2, "stderr: {}", output.stderr);
    assert!(
        results[0].contains("Bash(echo *) from claude-user"),
        "{results:?}"
    );
    assert!(!results[0].contains("again\\n"), "bash ran: {results:?}");
    assert!(
        results[1].contains("Bash(touch *) from pi-global"),
        "{results:?}"
    );
    let last_request = output.requests.last().unwrap().to_string();
    assert!(
        last_request.contains("Blocked by permission rule Bash(echo *)")
            && last_request.contains("Blocked by permission rule Bash(touch *)"),
        "{last_request}"
    );
}

#[test]
fn read_deny_blocks_bash_cat_of_the_file_and_the_model_never_sees_it() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let sentinel = "E2E_SENTINEL_9f3c41d2";
    let claude = tempfile::tempdir().unwrap();
    let settings = json!({ "permissions": { "deny": ["Read(./.env)"] } });
    std::fs::write(claude.path().join("settings.json"), settings.to_string()).unwrap();
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["permissions"])
        .env("CLAUDE_CONFIG_DIR", claude.path().display().to_string())
        .file("work/.env", format!("SECRET={sentinel}\n"))
        .turns([
            Turn::tool_call("bash", json!({ "command": "cat .env" })),
            Turn::text("done"),
        ])
        .prompt("Show the env file")
        .run()
        .unwrap();
    let results: Vec<String> = output
        .tool_results("bash")
        .iter()
        .map(|e| e["result"]["content"].to_string())
        .collect();
    assert_eq!(results.len(), 1, "stderr: {}", output.stderr);
    assert!(
        results[0].contains("Read(./.env) from claude-user"),
        "{results:?}"
    );
    assert!(
        !results[0].contains(sentinel),
        "bash read .env: {results:?}"
    );
    let requests: Vec<String> = output.requests.iter().map(ToString::to_string).collect();
    assert_eq!(requests.len(), 2, "stderr: {}", output.stderr);
    assert!(
        requests[1].contains("Blocked by permission rule Read(./.env)"),
        "{}",
        requests[1]
    );
    assert!(
        requests.iter().all(|request| !request.contains(sentinel)),
        "the file content reached the model"
    );
    assert!(!output.stdout.contains(sentinel) && !output.stderr.contains(sentinel));
}
