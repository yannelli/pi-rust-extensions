mod common;

use std::sync::Arc;

use common::http::{MockHttp, Reply};
use common::{TestHost, codex_jwt, ctx, now_ms, temp_dir};
use pi_ext_subscription_router::state::State;
use pi_ext_subscription_router::usage::Urls;
use serde_json::{Value, json};

const CONFIG: &str = r#"{ "profiles": { "main": { "name": "Main", "chain": [
    { "model": "openai-codex/gpt-5.5" }, { "model": "anthropic/claude-sonnet-5", "thinking": "high" } ] } } }"#;

fn agent_dir(name: &str, accounts: Option<Value>) -> std::path::PathBuf {
    let dir = temp_dir(name);
    std::fs::create_dir_all(dir.join("ext/subscription-router")).unwrap();
    std::fs::write(dir.join("ext/subscription-router.json"), CONFIG).unwrap();
    if let Some(accounts) = accounts {
        std::fs::write(
            dir.join("ext/subscription-router/accounts.json"),
            accounts.to_string(),
        )
        .unwrap();
    }
    dir
}

fn host_for(dir: &std::path::Path, mock: &MockHttp) -> TestHost {
    let urls = Urls {
        codex: format!("{}/wham/usage", mock.url),
        anthropic: format!("{}/api/oauth/usage", mock.url),
    };
    let state = Arc::new(State::with_urls(dir, urls));
    TestHost::start(pi_ext_subscription_router::extension(state))
}

fn model(provider: &str, id: &str) -> Value {
    json!({ "provider": provider, "id": id, "api": "openai-codex-responses", "baseUrl": "https://chatgpt.com/backend-api" })
}

/// Answers model lookups, auth, and dialogs the way a pi with both providers logged in would.
fn pi(method: &str, params: &Value) -> Value {
    match (method, params["method"].as_str()) {
        ("pi.call", Some("find")) => model(
            params["args"][0].as_str().unwrap(),
            params["args"][1].as_str().unwrap(),
        ),
        ("pi.call", Some("hasConfiguredAuth")) => json!(true),
        ("pi.call", Some("getAll")) => json!([
            model("openai-codex", "gpt-5.5"),
            model("anthropic", "claude-sonnet-5")
        ]),
        ("auth.getProviderAuth", _) => {
            json!({ "apiKey": codex_jwt("acct-1"), "headers": {}, "source": "oauth" })
        }
        ("model.set", _) => json!(true),
        _ => Value::Null,
    }
}

#[tokio::test]
async fn initialize_declares_tool_commands_and_virtual_models() {
    let mock = MockHttp::start(vec![]).await;
    let dir = agent_dir("init", None);
    let mut host = host_for(&dir, &mock);
    let init = host.initialize(dir.to_str().unwrap()).await;
    assert_eq!(init["protocolVersion"], 1);
    assert_eq!(init["tools"][0]["name"], "subscription_usage");
    assert_eq!(init["tools"][0]["label"], "Subscription Usage");
    let commands: Vec<&str> = init["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(commands, ["router", "codex-status", "codex-switch"]);
    assert_eq!(init["virtualModels"][0]["provider"], "router");
    assert_eq!(init["virtualModels"][0]["id"], "main");
    assert_eq!(init["virtualModels"][0]["name"], "Main");
    assert_eq!(
        init["virtualModels"][0]["thinkingLevels"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
    for event in [
        "session_start",
        "before_agent_start",
        "message_end",
        "after_provider_response",
        "agent_before_settle",
    ] {
        assert!(
            init["events"].as_array().unwrap().contains(&json!(event)),
            "{event}"
        );
    }
    assert!(init.get("providers").is_none());
    host.finish().await.unwrap();
}

#[tokio::test]
async fn retry_route_after_confirmed_429_returns_next_chain_model() {
    let reset = now_ms() / 1000 + 7200;
    let exhausted = json!({ "rate_limit": { "primary_window": { "used_percent": 100, "limit_window_seconds": 18000, "reset_at": reset } } });
    let mock = MockHttp::start(vec![Reply::json(200, exhausted)]).await;
    let dir = agent_dir("route", None);
    let mut host = host_for(&dir, &mock);
    host.initialize(dir.to_str().unwrap()).await;
    let failed = json!({ "role": "assistant", "content": [], "api": "openai-codex-responses", "provider": "openai-codex",
        "model": "gpt-5.5", "stopReason": "error", "errorMessage": "429 Too Many Requests: The usage limit has been reached",
        "timestamp": now_ms() - 5_000 });
    let request = json!({ "provider": "router", "id": "main", "request": {
        "model": { "provider": "router", "id": "main" }, "thinkingLevel": "medium", "reason": "retry",
        "failed": { "model": { "provider": "openai-codex", "id": "gpt-5.5" }, "thinkingLevel": "medium", "message": failed },
        "messages": [] }, "ctx": ctx(Some(("router", "main"))) });
    host.request(1, "virtualModel.route", request).await;
    let (response, seen) = host.serve_until(1, pi).await;
    assert_eq!(
        response["result"],
        json!({ "model": { "provider": "anthropic", "id": "claude-sonnet-5" }, "thinkingLevel": "high" })
    );
    let usage = &mock.requests()[0];
    assert_eq!(usage.path, "/wham/usage");
    assert_eq!(usage.headers["chatgpt-account-id"], "acct-1");
    let entry = seen
        .iter()
        .find(|(m, _)| m == "session.appendEntry")
        .expect("decision entry");
    assert_eq!(entry.1["customType"], "router:decision");
    assert_eq!(entry.1["data"]["reason"], "retry");
    assert_eq!(entry.1["data"]["chosen"], "anthropic/claude-sonnet-5");
    assert!(
        entry.1["data"]["blocked"][0]["reason"]
            .as_str()
            .unwrap()
            .starts_with("5h exhausted, resets")
    );
    host.request(2, "tool.execute", json!({ "toolCallId": "t1", "name": "subscription_usage", "params": {}, "ctx": ctx(Some(("router", "main"))) })).await;
    let (tool, _) = host.serve_until(2, pi).await;
    let text = tool["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("blocked openai-codex: 5h until"), "{text}");
    assert_eq!(
        tool["result"]["details"]["blocks"][0]["provider"],
        "openai-codex"
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn slot_registration_and_message_end_failover() {
    let mock = MockHttp::start(vec![Reply::json(500, json!({}))]).await;
    let access = codex_jwt("acct-2");
    let accounts = json!({ "version": 1, "accounts": { "openai-codex-2": {
        "type": "oauth", "access": access, "refresh": "r", "expires": now_ms() + 3_600_000, "accountId": "acct-2" } } });
    let dir = agent_dir("slots", Some(accounts));
    let mut host = host_for(&dir, &mock);
    host.initialize(dir.to_str().unwrap()).await;
    let start = json!({ "name": "session_start", "event": { "type": "session_start", "reason": "startup" }, "ctx": ctx(Some(("openai-codex-2", "gpt-5.5"))) });
    host.request(1, "event", start).await;
    let mut registered = None;
    let mut seen_count = 0;
    while registered.is_none() {
        let message = host.recv().await;
        let method = message["method"].as_str().unwrap_or_default().to_owned();
        if method == "provider.register" {
            registered = Some(message["params"].clone());
        }
        if let Some(id) = message.get("id").filter(|_| !method.is_empty()) {
            host.answer(id, pi(&method, &message["params"])).await;
        }
        seen_count += 1;
        assert!(seen_count < 50, "no provider.register");
    }
    let registered = registered.unwrap();
    assert_eq!(registered["name"], "openai-codex-2");
    let config = &registered["config"];
    assert_eq!(config["name"], "OpenAI Codex #2");
    assert_eq!(config["api"], "openai-codex-responses");
    assert_eq!(config["baseUrl"], "https://chatgpt.com/backend-api");
    assert_eq!(config["apiKey"], access);
    assert_eq!(config["models"].as_array().unwrap().len(), 1);
    assert_eq!(config["models"][0]["provider"], "openai-codex-2");
    assert!(config.get("oauth").is_none());

    let slot_ctx = ctx(Some(("openai-codex-2", "gpt-5.5")));
    host.request(2, "event", json!({ "name": "after_provider_response", "event": { "type": "after_provider_response", "status": 429, "headers": {} }, "ctx": slot_ctx })).await;
    host.serve_until(2, pi).await;
    let message = json!({ "role": "assistant", "content": [], "provider": "openai-codex-2", "model": "gpt-5.5", "stopReason": "error", "errorMessage": "Rate limit reached" });
    host.request(3, "event", json!({ "name": "message_end", "event": { "type": "message_end", "message": message }, "ctx": slot_ctx })).await;
    let (response, seen) = host.serve_until(3, pi).await;
    let set = seen
        .iter()
        .find(|(m, _)| m == "model.set")
        .expect("model.set");
    assert_eq!(
        set.1,
        json!({ "provider": "openai-codex", "id": "gpt-5.5" })
    );
    assert_eq!(
        response["result"]["result"]["message"]["errorMessage"],
        "HTTP 429: Rate limit reached"
    );
    let notice = seen.iter().find(|(m, _)| m == "ui.notify").expect("notice");
    assert_eq!(
        notice.1["message"],
        "Codex #2 returned HTTP 429. Retrying with Codex #1."
    );
    host.finish().await.unwrap();
}

/// `/router add` for Anthropic, with `auth.login` answering `login`.
async fn add_anthropic(name: &str, login: Value) -> (Vec<(String, Value)>, std::path::PathBuf) {
    let mock = MockHttp::start(vec![]).await;
    let dir = agent_dir(name, None);
    let mut host = host_for(&dir, &mock);
    host.initialize(dir.to_str().unwrap()).await;
    let run = json!({ "name": "router", "args": "add", "ctx": ctx(None) });
    host.request(1, "command.run", run).await;
    let (_, seen) = host
        .serve_until(1, |method, params| match method {
            "ui.select" => json!("Anthropic"),
            "auth.login" => login.clone(),
            _ => pi(method, params),
        })
        .await;
    host.finish().await.unwrap();
    (seen, dir)
}

#[tokio::test]
async fn router_add_logs_in_through_pi_and_keeps_no_token() {
    let (seen, dir) = add_anthropic("add-pi", json!(true)).await;
    let register = seen
        .iter()
        .find(|(m, _)| m == "provider.register")
        .expect("provider.register");
    assert_eq!(register.1["name"], "anthropic-2");
    assert_eq!(
        register.1["config"]["oauth"],
        json!({ "builtin": "anthropic", "name": "Anthropic #2" })
    );
    assert!(register.1["config"].get("apiKey").is_none());
    let login = seen
        .iter()
        .find(|(m, _)| m == "auth.login")
        .expect("auth.login");
    assert_eq!(login.1, json!({ "provider": "anthropic-2" }));
    let notice = seen.iter().find(|(m, _)| m == "ui.notify").expect("notice");
    assert_eq!(
        notice.1["message"],
        "Added Anthropic #2 as provider anthropic-2."
    );
    let file = std::fs::read_to_string(dir.join("ext/subscription-router/accounts.json")).unwrap();
    let file: Value = serde_json::from_str(&file).unwrap();
    assert_eq!(file["accounts"]["anthropic-2"], json!({ "type": "pi" }));
}

#[tokio::test]
async fn a_cancelled_pi_login_removes_the_new_slot() {
    let (seen, dir) = add_anthropic("add-cancel", json!(false)).await;
    let methods: Vec<&str> = seen.iter().map(|(m, _)| m.as_str()).collect();
    let login = methods
        .iter()
        .position(|m| *m == "auth.login")
        .expect("auth.login");
    assert!(
        methods[login..].contains(&"provider.unregister"),
        "{methods:?}"
    );
    assert!(!methods.contains(&"ui.notify"), "{methods:?}");
    assert!(!dir.join("ext/subscription-router/accounts.json").exists());
}
