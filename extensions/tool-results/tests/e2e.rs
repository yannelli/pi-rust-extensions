#![cfg(unix)]

use pi_ext_e2e::{Run, Turn, cargo_bin, json, pi_bin_or_skip};
use pi_ext_tool_results::store::result_id;

/// The text of the tool message the model got in request `n`.
fn tool_message(requests: &[serde_json::Value], n: usize) -> String {
    let messages = requests[n]["messages"].as_array().unwrap();
    let message = messages.iter().rev().find(|m| m["role"] == "tool").unwrap();
    match &message["content"] {
        serde_json::Value::String(text) => text.clone(),
        other => other[0]["text"].as_str().unwrap_or_default().to_owned(),
    }
}

#[test]
fn bash_output_is_cleaned_stored_and_read_back() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let id = result_id("call_0_0");
    let command = "printf '\\033[31mred\\033[0m\\n'; seq -f 'row %g of the test output' 1 1200";
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["tool-results"])
        .settings(json!({}))
        .file("agent/ext/tool-results.json", r#"{ "threshold": 5000 }"#)
        .turns([
            Turn::tool_call("bash", json!({ "command": command })),
            Turn::tool_call("tool_result_get", json!({ "result_id": id, "mode": "search", "pattern": "row 850 ", "context_chars": 6 })),
            Turn::text("done"),
        ])
        .prompt("run it")
        .run()
        .unwrap();
    let first = tool_message(&output.requests, 1);
    assert!(
        first.starts_with("red\nrow 1 of the test output\n"),
        "{first}\nstderr:\n{}",
        output.stderr
    );
    assert!(
        first.contains(&format!("with result_id \"{id}\"")),
        "{first}"
    );
    assert!(first.len() < 8000, "{}", first.len());
    let second = tool_message(&output.requests, 2);
    assert!(
        second.contains("--- line 851\n…utput\nrow 850 of the…"),
        "{second}\nstderr:\n{}",
        output.stderr
    );
}
