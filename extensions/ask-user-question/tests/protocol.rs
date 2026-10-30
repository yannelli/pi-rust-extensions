mod common;

use common::{TestHost, emitted};
use pi_ext_ask_user_question::config::Config;
use pi_ext_ask_user_question::extension;
use serde_json::{Value, json};

fn ui_ctx(has_ui: bool) -> Value {
    json!({ "cwd": "/work", "hasUI": has_ui, "mode": "tui", "flags": {} })
}

fn questions() -> Value {
    json!([{
        "question": "Which db?", "header": "DB",
        "options": [
            { "label": "Postgres", "description": "server" },
            { "label": "SQLite", "description": "file", "preview": "PREVIEW" }
        ]
    }])
}

async fn start() -> TestHost {
    let mut host = TestHost::start(extension(&Config::default()));
    let init = host.initialize().await;
    assert_eq!(init["tools"][0]["name"], "ask_user_question");
    assert_eq!(init["tools"][0]["label"], "Ask User Question");
    host
}

async fn execute(host: &mut TestHost, id: u64, ctx: Value) {
    let params = json!({ "toolCallId": "call-1", "name": "ask_user_question", "params": { "questions": questions() }, "ctx": ctx });
    host.request(id, "tool.execute", params).await;
}

#[tokio::test]
async fn panel_answer_through_keys() {
    let mut host = start().await;
    execute(&mut host, 5, ui_ctx(true)).await;
    let mut seen = Vec::new();
    let custom = host.pump_until("ui.custom", &mut seen).await;
    let surface = custom["params"]["surface"].clone();
    let lines = custom["params"]["lines"].as_array().unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l.as_str().unwrap().contains("Which db?"))
    );
    host.notify("ui.resize", json!({ "surface": surface, "width": 60 }))
        .await;
    host.pump_until("ui.update", &mut seen).await;
    host.notify("ui.key", json!({ "surface": surface, "data": "\u{1b}[B" }))
        .await;
    let update = host.pump_until("ui.update", &mut seen).await;
    let text = update["params"]["lines"].to_string();
    assert!(text.contains("2. SQLite"), "{text}");
    host.notify("ui.key", json!({ "surface": surface, "data": "\r" }))
        .await;
    let response = host.pump_response(5, &mut seen).await;
    let result = &response["result"];
    assert_eq!(
        result["content"][0]["text"],
        "User has answered your questions: \"Which db?\"=\"SQLite\". selected preview: PREVIEW. You can now continue with the user's answers in mind."
    );
    assert_eq!(result["details"]["answers"][0]["kind"], "option");
    assert_eq!(result["details"]["answers"][0]["preview"], "PREVIEW");

    let subscribe = seen
        .iter()
        .find(|m| m["method"] == "events.subscribe")
        .unwrap();
    assert_eq!(subscribe["params"]["channel"], "rpiv:ask-user:answer");
    let prompt = emitted(&seen, "rpiv:ask-user:prompt");
    assert_eq!(prompt[0]["toolCallId"], "call-1");
    assert_eq!(
        prompt[0]["questions"][0]["options"][1],
        json!({ "label": "SQLite", "description": "file", "hasPreview": true })
    );
    let blocked: Vec<bool> = emitted(&seen, "rpiv:ask-user:blocked")
        .iter()
        .map(|d| d["active"].as_bool().unwrap())
        .collect();
    assert_eq!(blocked, [true, false]);
    let bell = seen
        .iter()
        .find(|m| m["method"] == "ui.writeTerminal")
        .unwrap();
    assert_eq!(bell["params"]["data"], "\u{7}");
    assert!(seen.iter().any(|m| m["method"] == "ui.close"));
    host.finish().await.unwrap();
}

#[tokio::test]
async fn escape_declines() {
    let mut host = start().await;
    execute(&mut host, 5, ui_ctx(true)).await;
    let mut seen = Vec::new();
    let custom = host.pump_until("ui.custom", &mut seen).await;
    let surface = custom["params"]["surface"].clone();
    host.notify("ui.key", json!({ "surface": surface, "data": "\u{1b}" }))
        .await;
    let response = host.pump_response(5, &mut seen).await;
    assert_eq!(
        response["result"]["content"][0]["text"],
        "User declined to answer questions"
    );
    assert_eq!(response["result"]["details"]["cancelled"], true);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn inbound_answer_closes_the_panel() {
    let mut host = start().await;
    execute(&mut host, 5, ui_ctx(true)).await;
    let mut seen = Vec::new();
    host.pump_until("ui.custom", &mut seen).await;
    let bad = json!({ "toolCallId": "call-1", "requestId": "r0", "answers": [] });
    host.notify(
        "events.message",
        json!({ "channel": "rpiv:ask-user:answer", "data": bad }),
    )
    .await;
    let verdict = host.pump_until("events.emit", &mut seen).await;
    assert_eq!(verdict["params"]["data"]["code"], "invalid_answer");
    host.answer(&verdict["id"], Value::Null).await;
    let good = json!({ "toolCallId": "call-1", "requestId": "r1", "answers": [{ "questionIndex": 0, "text": "DuckDB" }] });
    host.notify(
        "events.message",
        json!({ "channel": "rpiv:ask-user:answer", "data": good }),
    )
    .await;
    let response = host.pump_response(5, &mut seen).await;
    assert_eq!(
        response["result"]["content"][0]["text"],
        "User has answered your questions: \"Which db?\"=\"DuckDB\". You can now continue with the user's answers in mind."
    );
    let verdicts = emitted(&seen, "rpiv:ask-user:answer-result");
    assert!(
        verdicts.contains(&&json!({ "ok": true, "requestId": "r1", "toolCallId": "call-1" })),
        "{verdicts:?}"
    );
    assert!(seen.iter().any(|m| m["method"] == "ui.close"));
    host.finish().await.unwrap();
}

#[tokio::test]
async fn no_ui_and_validation_errors() {
    let mut host = start().await;
    execute(&mut host, 5, ui_ctx(false)).await;
    let response = host.expect_response(5).await;
    assert_eq!(
        response["result"]["details"],
        json!({ "answers": [], "cancelled": true, "error": "no_ui" })
    );
    let params = json!({ "toolCallId": "c2", "name": "ask_user_question", "ctx": ui_ctx(true),
        "params": { "questions": [{ "question": "Q?", "header": "H", "options": [{ "label": "Other", "description": "" }, { "label": "b", "description": "" }] }] } });
    host.request(6, "tool.execute", params).await;
    let response = host.expect_response(6).await;
    assert_eq!(response["result"]["details"]["error"], "reserved_label");
    host.finish().await.unwrap();
}

#[tokio::test]
async fn before_agent_start_strips_and_restores() {
    let mut host = start().await;
    let event = json!({ "type": "before_agent_start", "prompt": "hi", "systemPromptOptions": {} });
    host.request(
        7,
        "event",
        json!({ "name": "before_agent_start", "event": event, "ctx": ui_ctx(false) }),
    )
    .await;
    let (id, _) = host.expect_request("tools.getActive").await;
    host.answer(&id, json!(["read", "ask_user_question"])).await;
    let (id, params) = host.expect_request("tools.setActive").await;
    assert_eq!(params, json!({ "names": ["read"] }));
    host.answer(&id, Value::Null).await;
    host.expect_response(7).await;
    host.request(
        8,
        "event",
        json!({ "name": "before_agent_start", "event": event, "ctx": ui_ctx(true) }),
    )
    .await;
    let (id, _) = host.expect_request("tools.getActive").await;
    host.answer(&id, json!(["read"])).await;
    let (id, params) = host.expect_request("tools.setActive").await;
    assert_eq!(params, json!({ "names": ["read", "ask_user_question"] }));
    host.answer(&id, Value::Null).await;
    host.expect_response(8).await;
    host.request(
        9,
        "event",
        json!({ "name": "before_agent_start", "event": event, "ctx": ui_ctx(true) }),
    )
    .await;
    let (id, _) = host.expect_request("tools.getActive").await;
    host.answer(&id, json!(["ask_user_question"])).await;
    host.expect_response(9).await;
    host.finish().await.unwrap();
}

#[tokio::test]
async fn rpc_mode_walks_dialogs() {
    let mut host = start().await;
    let ctx = json!({ "cwd": "/work", "hasUI": true, "mode": "rpc" });
    execute(&mut host, 5, ctx).await;
    let mut seen = Vec::new();
    let select = host.pump_until("ui.select", &mut seen).await;
    assert_eq!(
        select["params"]["title"],
        "[DB] Which db?\n\n--- 2. SQLite preview ---\nPREVIEW"
    );
    host.answer(&select["id"], json!("1. Postgres — server"))
        .await;
    let response = host.pump_response(5, &mut seen).await;
    assert!(
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("\"Which db?\"=\"Postgres\"")
    );
    assert!(!seen.iter().any(|m| m["method"] == "ui.custom"));
    host.finish().await.unwrap();
}
