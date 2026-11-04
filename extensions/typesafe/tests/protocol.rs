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
    let (mut responded, mut subscribed) = (false, false);
    while !(responded && subscribed) {
        let message = host.recv().await;
        if message["method"] == "events.subscribe" {
            assert_eq!(message["params"]["channel"], "typesafe:decision:request");
            host.answer(&message["id"], Value::Null).await;
            subscribed = true;
        } else {
            assert_eq!(message["id"], 1, "unexpected message {message}");
            responded = true;
        }
    }
    (host, init)
}

fn decision_listing() -> Value {
    let decisions = json!(["/api/alpha/decisions"]);
    json!({ "data": [
        { "id": "typesafe/jev-1.13", "context_length": 262144, "pricing": { "prompt": "0.00000005" }, "endpoints": decisions },
        { "id": "typesafe/jev-1.14", "context_length": 262144, "pricing": { "prompt": "0.000000042" }, "endpoints": decisions },
        { "id": "~typesafe/jev-latest", "pricing": { "prompt": "0" }, "endpoints": decisions },
        { "id": "vendor/extract", "endpoints": ["/api/alpha/extractions"] }
    ]})
}

fn config(dir: &TempDir) -> Value {
    let text = std::fs::read_to_string(dir.0.join("ext/typesafe.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[tokio::test]
async fn models_picker_saves_and_applies_the_model() {
    let api = MockApi::start(vec![
        Reply::json(200, &decision_listing()),
        Reply::json(200, &answer()),
    ])
    .await;
    let dir = TempDir::new("models");
    std::fs::create_dir_all(dir.0.join("ext")).unwrap();
    std::fs::write(
        dir.0.join("ext/typesafe.json"),
        r#"{ "maxRequestsPerDay": 5 }"#,
    )
    .unwrap();
    let env = Env::fixed([
        ("PI_TYPESAFE_ENABLED", "1"),
        ("PI_TYPESAFE_BACKEND", "openrouter"),
        ("OPENROUTER_API_KEY", KEY),
        ("PI_TYPESAFE_BASE_URL", api.url.as_str()),
    ]);
    let (mut host, _) = start(&dir, env, true).await;
    host.request(
        2,
        "command.run",
        json!({ "name": "typesafe", "args": "models", "ctx": ctx(true) }),
    )
    .await;
    let (id, params) = host.expect_request("ui.select").await;
    assert_eq!(
        params["options"],
        json!([
            "~typesafe/jev-latest · free",
            "* typesafe/jev-1.13 · 262K ctx · $0.05/M in",
            "typesafe/jev-1.14 · 262K ctx · $0.042/M in"
        ])
    );
    host.answer(&id, params["options"][2].clone()).await;
    let (id, params) = host.expect_request("ui.notify").await;
    assert_eq!(
        params["message"],
        "TypeSafe model set to typesafe/jev-1.14."
    );
    host.answer(&id, Value::Null).await;
    host.expect_response(2).await;
    assert_eq!(
        config(&dir),
        json!({ "maxRequestsPerDay": 5, "model": "typesafe/jev-1.14", "priceUsdPerMtok": 0.042 })
    );

    let execute = json!({ "toolCallId": "c1", "name": "typesafe_evaluate", "params": sample_args(), "ctx": ctx(true) });
    host.request(3, "tool.execute", execute).await;
    let _ = host.expect_notification("tool.update").await;
    assert!(host.expect_response(3).await["result"].is_object());
    let requests = api.requests();
    assert_eq!(
        requests[0].path,
        "/api/v1/models?output_modalities=decisions"
    );
    assert_eq!(requests[0].authorization, None);
    let sent: Value = serde_json::from_str(&requests[1].body).unwrap();
    assert_eq!(sent["model"], "typesafe/jev-1.14");
}

/// Runs `/typesafe <args>` without a UI and returns the status message text.
async fn run_headless(host: &mut TestHost, id: u64, args: &str) -> String {
    let params = json!({ "name": "typesafe", "args": args, "ctx": ctx(false) });
    host.request(id, "command.run", params).await;
    let (message, params) = host.expect_request("session.sendMessage").await;
    assert_eq!(params["customType"], "typesafe-status");
    host.answer(&message, Value::Null).await;
    host.expect_response(id).await;
    params["content"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn models_list_headless_and_backend_saves_config() {
    let api = MockApi::start(vec![Reply::json(200, &decision_listing())]).await;
    let dir = TempDir::new("backend");
    let env = Env::fixed([
        ("PI_TYPESAFE_BACKEND", "openrouter"),
        ("PI_TYPESAFE_BASE_URL", api.url.as_str()),
    ]);
    let (mut host, _) = start(&dir, env, false).await;
    let host = &mut host;
    let listed = run_headless(host, 2, "models").await;
    assert_eq!(
        listed,
        "OpenRouter models (* current):\n~typesafe/jev-latest · free\n* typesafe/jev-1.13 · 262K ctx · $0.05/M in\ntypesafe/jev-1.14 · 262K ctx · $0.042/M in"
    );
    let shown = run_headless(host, 3, "backend").await;
    assert!(
        shown.starts_with("TypeSafe backend: openrouter. Valid backends: typesafe, openrouter"),
        "{shown}"
    );
    let refused = run_headless(host, 4, "backend nope").await;
    assert!(
        refused.starts_with("Unknown judgment backend \"nope\""),
        "{refused}"
    );
    let saved = run_headless(host, 5, "backend liquid").await;
    assert!(saved.starts_with("Saved backend liquid to "), "{saved}");
    assert!(
        saved.ends_with(
            "Run /reload to apply it. PI_TYPESAFE_BACKEND is set to openrouter and overrides it."
        ),
        "{saved}"
    );
    assert_eq!(config(&dir), json!({ "backend": "liquid" }));
}

#[tokio::test]
async fn bus_replies_disabled_then_answers_once_enabled() {
    let api = MockApi::start(vec![Reply::json(200, &answer())]).await;
    let dir = TempDir::new("bus");
    let env = Env::fixed([
        ("TYPESAFE_API_KEY", KEY),
        ("PI_TYPESAFE_BASE_URL", api.url.as_str()),
    ]);
    let (mut host, _) = start(&dir, env, true).await;
    let message = |id: &str| json!({ "channel": "typesafe:decision:request", "data": { "id": id, "request": sample_args() } });
    host.notify("events.message", message("r1")).await;
    let (id, params) = host.expect_request("events.emit").await;
    assert_eq!(params["channel"], "typesafe:decision:reply");
    assert_eq!(
        params["data"],
        json!({ "id": "r1", "ok": false, "code": "disabled", "error": "TypeSafe is disabled. Ask the operator to run /typesafe enable." })
    );
    host.answer(&id, Value::Null).await;
    assert!(api.requests().is_empty());

    host.request(
        2,
        "command.run",
        json!({ "name": "typesafe", "args": "enable", "ctx": ctx(true) }),
    )
    .await;
    let (id, _) = host.expect_request("ui.confirm").await;
    host.answer(&id, json!(true)).await;
    let (id, _) = host.expect_request("ui.notify").await;
    host.answer(&id, Value::Null).await;
    host.expect_response(2).await;

    host.notify("events.message", message("r2")).await;
    let (id, params) = host.expect_request("events.emit").await;
    let data = &params["data"];
    assert_eq!((&data["id"], &data["ok"]), (&json!("r2"), &json!(true)));
    assert_eq!(data["result"]["answers"]["urgent"]["noul"], 0.8);
    assert!(data["result"]["elapsedMs"].is_u64());
    assert!(!params.to_string().contains(KEY));
    host.answer(&id, Value::Null).await;
    assert_eq!(api.requests().len(), 1);
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
