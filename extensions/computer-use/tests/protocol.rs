mod common;

use common::{TestHost, ctx};
use pi_ext_computer_use::{TOOL_NAMES, extension, state_unavailable_message};
use serde_json::{Value, json};

fn execute(id: &str, name: &str, params: Value) -> Value {
    json!({ "toolCallId": id, "name": name, "params": params, "ctx": ctx() })
}

fn text(response: &Value) -> &str {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
}

fn observation_entry() -> Value {
    let root = json!({
        "ref": "@e1", "role": "window", "label": "Notes", "path": [], "children": [
            { "ref": "@e2", "role": "button", "label": "Save", "canPress": true, "path": [0],
              "frame": { "x": 10.0, "y": 20.0, "w": 40.0, "h": 20.0 }, "children": [] },
            { "ref": "@e3", "role": "text_field", "label": "Title", "value": "Draft", "canSetValue": true, "path": [1], "children": [] }
        ]
    });
    json!({
        "type": "message",
        "message": {
            "role": "toolResult", "toolName": "observe_ui",
            "details": {
                "tool": "observe_ui",
                "target": { "app": "Notes", "pid": 42, "windowTitle": "Notes", "windowId": 7, "windowRef": "@r1",
                            "framePoints": { "x": 0.0, "y": 0.0, "w": 800.0, "h": 600.0 } },
                "capture": { "stateId": "s-restored", "width": 1600, "height": 1200, "scaleFactor": 2.0, "timestamp": 1 },
                "outline": { "lookId": "look-1", "root": root }
            }
        }
    })
}

#[tokio::test]
async fn initialize_registers_upstream_tools_and_command() {
    let mut host = TestHost::start(extension());
    let result = host.initialize().await;
    let names: Vec<&str> = result["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, TOOL_NAMES);
    let search = result["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "search_ui")
        .unwrap();
    assert_eq!(search["parameters"]["required"], json!(["stateId"]));
    assert_eq!(search["label"], "Search UI");
    assert_eq!(result["commands"][0]["name"], "computer-use");
    host.finish().await.unwrap();
}

#[tokio::test]
async fn unknown_state_fails_without_host_calls() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let params = execute(
        "c1",
        "search_ui",
        json!({ "stateId": "missing", "text": "Save" }),
    );
    host.request(1, "tool.execute", params).await;
    let response = host.expect_response(1).await;
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(text(&response), state_unavailable_message("missing"));
    assert_eq!(
        text(&response),
        "State 'missing' is unavailable or was evicted. Observe the root again."
    );
    for (id, tool) in [
        (2, "expand_ui"),
        (3, "inspect_ui"),
        (4, "act_ui"),
        (5, "wait_for"),
    ] {
        let params = json!({ "stateId": "missing", "ref": "@e1", "text": "x", "actions": [{ "action": "press", "ref": "@e1" }] });
        host.request(id, "tool.execute", execute("c", tool, params))
            .await;
        let response = host.expect_response(id).await;
        assert_eq!(
            text(&response),
            state_unavailable_message("missing"),
            "{tool}"
        );
    }
    host.finish().await.unwrap();
}

#[tokio::test]
async fn output_ref_read_needs_no_state() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    host.request(
        1,
        "tool.execute",
        execute("c1", "read_text", json!({ "ref": "@o9" })),
    )
    .await;
    let response = host.expect_response(1).await;
    assert_eq!(
        text(&response),
        "Output ref '@o9' is unavailable or was evicted. Rerun the focused query."
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn session_start_replays_branch_details() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let mut ev_ctx = ctx();
    ev_ctx["isProjectTrusted"] = json!(false);
    let event =
        json!({ "name": "session_start", "event": { "type": "session_start" }, "ctx": ev_ctx });
    host.request(1, "event", event).await;
    let (id, _) = host.expect_request("session.getBranch").await;
    host.answer(&id, json!([observation_entry()])).await;
    host.expect_response(1).await;

    let params = json!({ "stateId": "s-restored", "text": "save" });
    host.request(2, "tool.execute", execute("c2", "search_ui", params))
        .await;
    let response = host.expect_response(2).await;
    assert!(
        text(&response).starts_with("Found 1 outline match; returned 1."),
        "{response}"
    );
    assert!(text(&response).contains("@e2 button \"Save\" [exact]"));
    assert_eq!(response["result"]["details"]["matches"], json!(["@e2"]));

    let params = json!({ "stateId": "s-restored", "ref": "@e3" });
    host.request(3, "tool.execute", execute("c3", "read_text", params))
        .await;
    let response = host.expect_response(3).await;
    assert!(text(&response).starts_with("Title\nDraft"), "{response}");

    let params = json!({ "stateId": "s-restored", "ref": "@e1", "depth": 1 });
    host.request(4, "tool.execute", execute("c4", "expand_ui", params))
        .await;
    let response = host.expect_response(4).await;
    assert!(
        text(&response).contains("  @e3 text_field \"Title\" value=\"Draft\" [setValue]"),
        "{response}"
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn search_requires_a_predicate() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    host.request(
        1,
        "event",
        json!({ "name": "session_start", "event": {}, "ctx": ctx() }),
    )
    .await;
    let (id, method) = loop {
        let message = host.recv().await;
        if message["method"] == "pi.call" {
            host.answer(&message["id"], json!(false)).await;
            continue;
        }
        break (message["id"].clone(), message["method"].clone());
    };
    assert_eq!(method, "session.getBranch");
    host.answer(&id, json!([observation_entry()])).await;
    host.expect_response(1).await;
    host.request(
        2,
        "tool.execute",
        execute("c", "search_ui", json!({ "stateId": "s-restored" })),
    )
    .await;
    let response = host.expect_response(2).await;
    assert_eq!(
        text(&response),
        "search_ui requires text, role, or capability. Use observe_ui for a bounded overview."
    );
    host.finish().await.unwrap();
}
