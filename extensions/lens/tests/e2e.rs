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
