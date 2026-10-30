//! The todo extension against an in-memory host: tool, scope channel, widget, and panel keys.

mod common;

use common::{TestHost, ctx};
use pi_ext_sdk::style::strip;
use pi_ext_todo::config::Config;
use serde_json::{Value, json};

fn widget_text(params: &Value) -> String {
    let lines = params["lines"].as_array().cloned().unwrap_or_default();
    lines
        .iter()
        .map(|l| strip(l.as_str().unwrap_or_default()))
        .collect::<Vec<_>>()
        .join("\n")
}

async fn started(agent: &pi_ext_e2e::TempDir) -> TestHost {
    let ext_dir = agent.path().join("ext");
    std::fs::create_dir_all(&ext_dir).unwrap();
    std::fs::write(
        ext_dir.join("todo.json"),
        r#"{ "defaultVisibility": "all" }"#,
    )
    .unwrap();
    let mut host = TestHost::start(pi_ext_todo::extension(Config::default(), None));
    let init = host.initialize_in(agent.path().to_str().unwrap()).await;
    assert_eq!(init["tools"][0]["name"], "todo");
    assert_eq!(init["shortcuts"][0]["shortcut"], "ctrl+shift+t");
    let event =
        json!({ "name": "session_start", "event": { "type": "session_start" }, "ctx": ctx() });
    host.request(1, "event", event).await;
    host.response(1).await;
    host
}

#[tokio::test]
async fn tool_call_persists_state_and_scope_event_updates_widget() {
    let agent = pi_ext_e2e::TempDir::new().unwrap();
    let mut host = started(&agent).await;
    let params = json!({ "toolCallId": "c1", "name": "todo", "ctx": ctx(),
        "params": { "action": "create", "subject": "Write parser" } });
    host.request(2, "tool.execute", params).await;
    let response = host.response(2).await;
    assert_eq!(
        response["result"]["content"][0]["text"],
        "Created #1: Write parser (pending)"
    );
    assert_eq!(response["result"]["details"]["nextId"], 2);
    let entries = host.seen_method("session.appendEntry");
    assert_eq!(entries[0]["customType"], "rpiv-todo-state");
    assert_eq!(entries[0]["data"]["tasks"][0]["subject"], "Write parser");
    let widget = host
        .seen_method("ui.setWidget")
        .last()
        .cloned()
        .cloned()
        .unwrap();
    assert_eq!(widget["key"], "rpiv-todos");
    assert_eq!(widget["placement"], "aboveEditor");
    assert!(widget_text(&widget).contains("○ Write parser"), "{widget}");

    let child = json!({ "action": "create", "params": {}, "nextId": 2, "scope": "run-1",
        "tasks": [{ "id": 1, "subject": "Scan files", "status": "in_progress" }] });
    let data =
        json!({ "scope": "run-1", "label": "explore: scan", "status": "running", "state": child });
    host.notify(
        "events.message",
        json!({ "channel": "todo:scope", "data": data }),
    )
    .await;
    let widget = host
        .until(|m| m["method"] == "ui.setWidget" && m["params"].to_string().contains("explore"))
        .await;
    let text = widget_text(&widget["params"]);
    assert!(text.contains("◆ explore: scan · 0/1 · running"), "{text}");
    assert!(text.contains("◐ Scan files"), "{text}");
    let scopes = host.seen_method("session.appendEntry");
    assert_eq!(scopes.last().unwrap()["customType"], "pi-todo-scopes");
    assert!(!host.seen_method("events.subscribe").is_empty());
    host.answer(&widget["id"], Value::Null).await;
    host.finish().await;
}

#[tokio::test]
async fn panel_keys_add_and_complete_a_todo() {
    let agent = pi_ext_e2e::TempDir::new().unwrap();
    let mut host = started(&agent).await;
    host.request(
        3,
        "command.run",
        json!({ "name": "todos", "args": "", "ctx": ctx() }),
    )
    .await;
    let custom = host.method("ui.custom").await;
    let surface = custom["params"]["surface"].clone();
    for key in ["a", "Ship it", "\r"] {
        host.notify("ui.key", json!({ "surface": surface, "data": key }))
            .await;
    }
    let update = host
        .until(|m| m["method"] == "ui.update" && m["params"].to_string().contains("Created #1"))
        .await;
    assert!(widget_text(&update["params"]).contains("○ #1 Ship it"));
    for key in ["\x1b[B", "\r", "\r"] {
        host.notify("ui.key", json!({ "surface": surface, "data": key }))
            .await;
    }
    host.until(|m| {
        m["method"] == "ui.update" && m["params"].to_string().contains("in_progress → completed")
    })
    .await;
    let last = host
        .seen_method("session.appendEntry")
        .last()
        .cloned()
        .cloned()
        .unwrap();
    assert_eq!(last["data"]["tasks"][0]["status"], "completed");
    host.notify("ui.key", json!({ "surface": surface, "data": "q" }))
        .await;
    let close = host.method("ui.close").await;
    assert_eq!(close["params"]["surface"], surface);
    host.answer(&custom["id"], json!({ "result": null })).await;
    assert_eq!(host.response(3).await["result"], Value::Null);
    host.finish().await;
}

#[tokio::test]
async fn child_scope_tags_details_and_draws_no_widget() {
    let mut host = TestHost::start(pi_ext_todo::extension(
        Config::default(),
        Some("run-9".into()),
    ));
    host.initialize().await;
    let params = json!({ "toolCallId": "c1", "name": "todo", "ctx": ctx(),
        "params": { "action": "create", "subject": "Child work" } });
    host.request(2, "tool.execute", params).await;
    let response = host.response(2).await;
    assert_eq!(response["result"]["details"]["scope"], "run-9");
    assert!(host.seen_method("ui.setWidget").is_empty());
    host.finish().await;
}
