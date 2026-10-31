//! Each goal path that blocks or redirects the agent puts its reason in a request to the model.

use pi_ext_e2e::{Output, Run, Turn, cargo_bin, json, pi_bin_or_skip};
use serde_json::Value;

const OBJECTIVE: &str = "Write the release notes for version 2";

fn run(turns: Vec<Turn>) -> Option<Output> {
    pi_bin_or_skip()?;
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["goal"])
        .turns(turns)
        .prompt("Create a goal to write the release notes for version 2.")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    Some(output)
}

fn message_text(message: &Value) -> String {
    match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn is_auditor(request: &Value) -> bool {
    let messages = request["messages"].as_array().cloned().unwrap_or_default();
    messages
        .iter()
        .any(|message| message_text(message).contains("completion auditor for pi-goal"))
}

/// Index of the first agent request with a user or tool message that contains `needle`.
fn request_with(output: &Output, needle: &str) -> Option<usize> {
    output.requests.iter().position(|request| {
        let messages = request["messages"].as_array().cloned().unwrap_or_default();
        !is_auditor(request)
            && messages
                .iter()
                .filter(|message| message["role"] == "user" || message["role"] == "tool")
                .any(|message| message_text(message).contains(needle))
    })
}

fn dump(output: &Output) -> String {
    let requests: Vec<String> = output
        .requests
        .iter()
        .enumerate()
        .map(|(i, request)| {
            let messages = request["messages"].as_array().cloned().unwrap_or_default();
            let lines: Vec<String> = messages
                .iter()
                .filter(|message| message["role"] != "system")
                .map(|message| format!("  [{}] {}", message["role"], message_text(message)))
                .collect();
            let system = output.system_prompt(i);
            let status: Vec<&str> = system
                .lines()
                .filter(|l| {
                    l.starts_with("Status:") || l.starts_with("[PI GOAL") || l.contains("TASK")
                })
                .collect();
            format!("request {i}: system={status:?}\n{}", lines.join("\n"))
        })
        .collect();
    format!("{}\nstderr:\n{}", requests.join("\n"), output.stderr)
}

fn assert_sent(output: &Output, needles: &[&str]) {
    for needle in needles {
        assert!(
            request_with(output, needle).is_some(),
            "no request to the model contains {needle:?}\n{}",
            dump(output)
        );
    }
}

fn pause() -> Turn {
    Turn::tool_call(
        "update_goal",
        json!({"status": "paused", "reason": "Stop the test."}),
    )
}

#[test]
fn audit_rejection_reaches_the_model() {
    let finding = "Missing: the upgrade section for version 2.";
    let Some(output) = run(vec![
        Turn::tool_call("create_goal", json!({"objective": OBJECTIVE})),
        Turn::tool_call(
            "update_goal",
            json!({"status": "complete", "completion_summary": "Notes written."}),
        ),
        Turn::text(format!("{finding}\n<disapproved/>")),
        pause(),
        Turn::text("Paused."),
    ]) else {
        return;
    };
    assert!(is_auditor(&output.requests[2]), "{}", dump(&output));
    assert_sent(&output, &["[GOAL AUDIT REJECTED", finding]);
    let rejected = output.tool_result_text("update_goal").unwrap_or_default();
    assert!(rejected.contains(finding), "{}", dump(&output));
}

#[test]
fn task_gate_names_the_pending_tasks() {
    let Some(output) = run(vec![
        Turn::tool_call("create_goal", json!({"objective": OBJECTIVE})),
        Turn::tool_call(
            "set_goal_tasks",
            json!({"tasks": [{"id": "t1", "title": "Draft the upgrade section"}], "block_completion": true}),
        ),
        Turn::tool_call("update_goal", json!({"status": "complete"})),
        pause(),
        Turn::text("Paused."),
    ]) else {
        return;
    };
    assert_sent(&output, &["TASK GATE", "t1: Draft the upgrade section"]);
}

#[test]
fn blocked_tool_call_gives_the_stop_reason() {
    let reason = "Waiting for the changelog from the user.";
    let Some(output) = run(vec![
        Turn::tool_call("create_goal", json!({"objective": OBJECTIVE})),
        Turn::tool_call("update_goal", json!({"status": "paused", "reason": reason})),
        Turn::tool_call("bash", json!({"command": "echo more work"})),
        Turn::text("Stopped."),
    ]) else {
        return;
    };
    assert_sent(&output, &["The goal stopped this turn", reason]);
}

#[test]
fn continuation_says_what_remains() {
    let Some(output) = run(vec![
        Turn::tool_call("create_goal", json!({"objective": OBJECTIVE})),
        Turn::tool_call(
            "set_goal_tasks",
            json!({"tasks": [{"id": "t1", "title": "Draft the notes"}, {"id": "t2", "title": "Publish the notes"}]}),
        ),
        Turn::text("Planned."),
        pause(),
        Turn::text("Paused."),
    ]) else {
        return;
    };
    assert_sent(
        &output,
        &[
            "<pi_goal_continuation",
            "The goal is not complete.",
            "Remaining: 2 of 2 tasks: t1: Draft the notes; t2: Publish the notes",
        ],
    );
}
