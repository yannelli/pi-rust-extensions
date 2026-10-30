//! The extension under the real pi binary: tools, prompt section, and task listing.

use pi_ext_e2e::{Run, Turn, cargo_example, json, pi_bin_or_skip};

#[test]
fn bg_run_and_status_through_pi() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let binary = cargo_example("pi-ext-background-tasks", "background-tasks");
    let run = json!({"name": "E2E sleeper", "command": "sleep 5", "isAgent": false});
    let output = Run::new(binary)
        .args(["background-tasks"])
        .turns([
            Turn::tool_call("bg_run", run),
            Turn::tool_call("bg_status", json!({})),
            Turn::text("Started."),
        ])
        .prompt("Run a sleeper in the background")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    let started = output.tool_result_text("bg_run").unwrap_or_default();
    assert!(
        started.starts_with("Started background task E2E sleeper"),
        "{started}"
    );
    let status = output.tool_result_text("bg_status").unwrap_or_default();
    assert!(status.contains("E2E sleeper [running]"), "{status}");
    for tool in ["bg_run", "bg_status", "bg_logs", "bg_kill"] {
        assert!(output.tool_names(0).contains(&tool.to_string()), "{tool}");
    }
    assert!(output.system_prompt(0).contains("Background shell policy"));
    assert_eq!(output.final_text(), "Started.");
}
