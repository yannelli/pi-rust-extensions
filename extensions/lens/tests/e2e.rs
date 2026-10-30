use pi_ext_e2e::{Run, TempDir, Turn, cargo_bin, json, pi_bin_or_skip};

#[test]
fn lens_checks_writes_injects_findings_and_guards_edits_in_pi() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let config = TempDir::new().unwrap();
    let config_path = config.path().join("lens.json");
    std::fs::write(&config_path, json!({
        "autoDetect":false,
        "commands":[{"name":"fixture","language":"python","stage":"lint","command":"/bin/sh","args":["-c","printf 'a.py:1:1: error: lens-e2e-finding\\n'"]}]
    }).to_string()).unwrap();
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["lens"])
        .env("PI_LENS_CONFIG_PATH", config_path.to_string_lossy())
        .turns([
            Turn::tool_call("write", json!({"path":"a.py","content":"x = 1\n"})),
            Turn::tool_call(
                "edit",
                json!({"path":"a.py","oldText":"x = 1","newText":"x = 2"}),
            ),
            Turn::tool_call("read", json!({"path":"a.py"})),
            Turn::tool_call(
                "edit",
                json!({"path":"a.py","oldText":"x = 1","newText":"x = 2"}),
            ),
            Turn::tool_call(
                "lens_diagnostics",
                json!({"source":"session","severity":"error"}),
            ),
            Turn::text("done"),
        ])
        .prompt("Create, read and edit the fixture file. Show Lens diagnostics.")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    let diagnostics = output.tool_result_text("lens_diagnostics").unwrap();
    assert!(
        diagnostics.contains("lens-e2e-finding"),
        "{diagnostics}\n{}",
        output.stderr
    );
    let edits = output.tool_results("edit");
    assert_eq!(edits.len(), 2, "{}", output.stdout);
    assert_eq!(edits[0]["result"]["isError"], true);
    assert_ne!(edits[1]["result"]["isError"], true);
    assert!(
        output.requests.get(1).unwrap()["messages"]
            .to_string()
            .contains("lens-e2e-finding")
    );
    assert_eq!(output.final_text(), "done");
}

fn lens_run(config: serde_json::Value) -> (pi_ext_e2e::Output, std::time::Duration) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("lens.json");
    std::fs::write(&path, config.to_string()).unwrap();
    let write = |name: &str| Turn::tool_call("write", json!({"path":name,"content":"x = 1\n"}));
    let started = std::time::Instant::now();
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["lens"])
        .env("PI_LENS_CONFIG_PATH", path.to_string_lossy())
        .turns([
            write("a.py"),
            write("b.py"),
            write("c.py"),
            Turn::tool_call("lens_diagnostics", json!({"source":"session"})),
            Turn::text("done"),
        ])
        .prompt("Write three files.")
        .timeout(std::time::Duration::from_secs(120))
        .run()
        .unwrap();
    (output, started.elapsed())
}

#[cfg(unix)]
#[test]
fn stalled_lsp_does_not_block_writes_beyond_the_limit() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let (baseline, base_time) = lens_run(json!({"autoDetect":false}));
    assert!(baseline.status.success(), "{}", baseline.stderr);
    let sleep = format!("sleep 7{}", std::process::id());
    let stalled = json!({"autoDetect":false,"timeoutMs":800,"pipelineTimeoutMs":1000,
        "lsp":{"servers":[{"command":"/bin/sh","args":["-c",format!("exec {sleep}")],"languages":["python"]}]}});
    let (output, time) = lens_run(stalled);
    assert!(output.status.success(), "{}", output.stderr);
    let writes = output.tool_results("write");
    assert_eq!(writes.len(), 3, "{}", output.stdout);
    assert!(writes.iter().all(|item| item["result"]["isError"] != true));
    let diagnostics = output.tool_result_text("lens_diagnostics").unwrap();
    assert!(diagnostics.contains("timed out after"), "{diagnostics}");
    assert!(
        diagnostics.contains("Skipped: timed out earlier"),
        "{diagnostics}"
    );
    assert!(
        time.saturating_sub(base_time) < std::time::Duration::from_secs(3),
        "stalled run {time:?}, baseline {base_time:?}"
    );
    let left = || {
        let found = std::process::Command::new("pgrep")
            .args(["-f", &sleep])
            .output()
            .unwrap();
        String::from_utf8_lossy(&found.stdout).trim().to_owned()
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !left().is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let pids = left();
    let ps = std::process::Command::new("ps")
        .args([
            "-o",
            "pid,ppid,pgid,stat,command",
            "-p",
            &pids.replace('\n', ","),
        ])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned());
    assert!(pids.is_empty(), "{ps:?}\n{}", output.stderr);
    assert_eq!(output.final_text(), "done");
}
