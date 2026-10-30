mod common;

use common::{TempDir, TestHost, ctx};
use pi_ext_typesafe::credentials::Env;
use pi_ext_typesafe::extension_with;
use pi_ext_typesafe::mock_api::{MockApi, Reply};
use pi_ext_typesafe::state::State;
use serde_json::{Value, json};

const KEY: &str = "ts_test_0123456789abcdef";

fn answer() -> Value {
    json!({ "model": "jev-1.13", "usage": { "input_tokens": 40, "output_tokens": 2 }, "answers": {
        "category": { "type": "choice", "choice": "billing", "confidence": 0.9,
            "probabilities": { "billing": 0.9, "technical": 0.05, "other": 0.05 } },
        "urgent": { "type": "noul", "noul": 0.8 },
        "frustration": { "type": "score", "score": 0.6, "confidence": 0.5, "legend": {},
            "probabilities": { "0": 0.5, "1": 0.4, "2": 0.1 } }
    }})
}

fn sample_args() -> Value {
    json!({ "state": { "message": "charged twice" }, "questions": {
        "category": { "type": "choice", "options": ["billing", "technical", "other"] },
        "urgent": { "type": "noul", "instructions": "Today?" },
        "frustration": { "type": "score", "criteria": ["calm", "upset", "angry"] }
    }})
}

async fn start(dir: &TempDir, env: Env, has_ui: bool) -> (TestHost, Value) {
    let mut host = TestHost::start(extension_with(State::new(env, &dir.0)));
    let init = host.initialize(&dir.0, has_ui).await;
    host.request(
        1,
        "event",
        json!({ "name": "session_start", "event": {}, "ctx": ctx(has_ui) }),
    )
    .await;
    host.expect_response(1).await;
    (host, init)
}

#[tokio::test]
async fn tool_evaluates_against_mock_api_and_gate_closes() {
    let api = MockApi::start(vec![Reply::json(200, &answer())]).await;
    let dir = TempDir::new("tool");
    let env = Env::fixed([
        ("PI_TYPESAFE_ENABLED", "1"),
        ("TYPESAFE_API_KEY", KEY),
        ("PI_TYPESAFE_BASE_URL", api.url.as_str()),
    ]);
    let (mut host, init) = start(&dir, env, true).await;
    let tool = &init["tools"][0];
    assert_eq!(tool["name"], "typesafe_evaluate");
    assert_eq!(tool["label"], "TypeSafe");
    assert_eq!(
        tool["parameters"]["required"],
        json!(["state", "questions"])
    );
    assert_eq!(tool["promptGuidelines"].as_array().unwrap().len(), 5);
    assert_eq!(init["commands"][0]["name"], "typesafe");

    let execute = json!({ "toolCallId": "c1", "name": "typesafe_evaluate", "params": sample_args(), "ctx": ctx(true) });
    host.request(2, "tool.execute", execute.clone()).await;
    let update = host.expect_notification("tool.update").await;
    assert_eq!(
        update["partial"]["content"][0]["text"],
        "TypeSafe · 3 questions · external request"
    );
    let response = host.expect_response(2).await;
    let result = &response["result"];
    assert_eq!(
        result["details"]["answers"]["category"]["choice"],
        "billing"
    );
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["model"], "jev-1.13");
    let sent: Value = serde_json::from_str(&api.requests()[0].body).unwrap();
    assert_eq!(
        sent["questions"]["category"]["criteria"],
        json!({ "billing": null, "technical": null, "other": null })
    );

    host.request(
        3,
        "command.run",
        json!({ "name": "typesafe", "args": "disable", "ctx": ctx(true) }),
    )
    .await;
    let (id, params) = host.expect_request("ui.notify").await;
    assert!(
        params["message"]
            .as_str()
            .unwrap()
            .starts_with("TypeSafe disabled")
    );
    host.answer(&id, Value::Null).await;
    host.expect_response(3).await;
    host.request(4, "tool.execute", execute).await;
    let refused = host.expect_response(4).await;
    let message = refused["error"]["message"].as_str().unwrap();
    assert!(message.starts_with("TypeSafe is disabled. Ask the operator to run /typesafe enable"));
    assert_eq!(api.requests().len(), 1);
}

#[tokio::test]
async fn rejected_key_calls_out_once_and_status_is_headless() {
    let api = MockApi::start(vec![
        Reply::json(401, &json!({})),
        Reply::json(401, &json!({})),
    ])
    .await;
    let dir = TempDir::new("reject");
    let env = Env::fixed([
        ("PI_TYPESAFE_ENABLED", "1"),
        ("TYPESAFE_API_KEY", KEY),
        ("PI_TYPESAFE_BASE_URL", api.url.as_str()),
    ]);
    let (mut host, _) = start(&dir, env, false).await;
    let execute = json!({ "toolCallId": "c1", "name": "typesafe_evaluate", "params": sample_args(), "ctx": ctx(false) });
    host.request(2, "tool.execute", execute.clone()).await;
    let _ = host.expect_notification("tool.update").await;
    let (id, params) = host.expect_request("session.sendMessage").await;
    assert_eq!(params["customType"], "typesafe-status");
    assert!(params["content"].as_str().unwrap().starts_with(
        "TypeSafe is not authenticated (TypeSafe returned HTTP 401. Check TYPESAFE_API_KEY."
    ));
    host.answer(&id, Value::Null).await;
    let failed = host.expect_response(2).await;
    assert!(!failed["error"]["message"].as_str().unwrap().contains(KEY));
    host.request(3, "tool.execute", execute).await;
    let _ = host.expect_notification("tool.update").await;
    assert!(host.expect_response(3).await["error"].is_object());

    host.request(
        4,
        "command.run",
        json!({ "name": "typesafe", "args": "", "ctx": ctx(false) }),
    )
    .await;
    let (id, params) = host.expect_request("session.sendMessage").await;
    let status = params["content"].as_str().unwrap();
    assert!(
        status.starts_with("TypeSafe: enabled. TypeSafe backend: 127.0.0.1"),
        "{status}"
    );
    assert!(status.contains("was rejected."), "{status}");
    assert!(
        status.contains("Session 2/20 attempts, 0 successful, 2 failed"),
        "{status}"
    );
    host.answer(&id, Value::Null).await;
    host.expect_response(4).await;

    host.request(
        5,
        "command.complete",
        json!({ "name": "typesafe", "prefix": "login li" }),
    )
    .await;
    let items = host.expect_response(5).await;
    assert_eq!(items["result"][0]["value"], "login liquid");
}

#[tokio::test]
async fn login_reads_hidden_key_and_saves_owner_only() {
    let api = MockApi::start(vec![Reply::json(
        200,
        &json!({ "models": [{ "name": "jev-1.13" }] }),
    )])
    .await;
    let dir = TempDir::new("login");
    let env = Env::fixed([("PI_TYPESAFE_BASE_URL", api.url.as_str())]);
    let (mut host, _) = start(&dir, env, true).await;
    host.request(
        2,
        "command.run",
        json!({ "name": "typesafe", "args": "login", "ctx": ctx(true) }),
    )
    .await;
    let (custom_id, params) = host.expect_request("ui.custom").await;
    let surface = params["surface"].clone();
    host.notify("ui.key", json!({ "surface": surface, "data": KEY }))
        .await;
    let update = host.expect_notification("ui.update").await;
    assert!(!update.to_string().contains(KEY));
    host.notify("ui.key", json!({ "surface": surface, "data": "\r" }))
        .await;
    let _ = host.expect_notification("ui.close").await;
    host.answer(&custom_id, json!({ "result": null })).await;
    let (id, params) = host.expect_request("ui.notify").await;
    let message = params["message"].as_str().unwrap();
    assert!(
        message.starts_with("TypeSafe key verified (1 model available) and saved to"),
        "{message}"
    );
    host.answer(&id, Value::Null).await;
    host.expect_response(2).await;
    assert_eq!(
        api.requests()[0].authorization.as_deref(),
        Some(format!("Bearer {KEY}").as_str())
    );
    let path = dir.0.join("pi-typesafe/auth.json");
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(stored["keys"]["typesafe"], KEY);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
