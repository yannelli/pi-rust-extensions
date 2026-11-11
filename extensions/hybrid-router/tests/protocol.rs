//! `hybrid/auto` over the extension protocol, against a scripted pi with a local `box/qwen`
//! (chat model and classifier) and a remote `far/big`.

use std::path::PathBuf;
use std::sync::Arc;

use pi_ext_hybrid_router::config::{self, Config, ModelRef};
use pi_ext_hybrid_router::extension;
use pi_ext_hybrid_router::route::{Router, latest_prompt};
use pi_ext_hybrid_router::setup::parse_version;
use pi_ext_sdk::test_util::{TestHost, ctx};
use serde_json::{Value, json};

fn config() -> Config {
    Config {
        local: ModelRef::parse("box/qwen").unwrap(),
        remote: ModelRef::parse("far/big").unwrap(),
        busy_load_per_core: None,
    }
}

fn idle() -> f64 {
    0.1
}

fn busy() -> f64 {
    4.0
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hybrid-router-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn model(provider: &str, id: &str) -> Value {
    json!({ "provider": provider, "id": id, "api": "openai-completions" })
}

/// Answers the way pi would, rating prompts that mention "refactor" complex. `classifier` is
/// false for pi releases whose `findOfType` misses classifiers.
fn pi(method: &str, params: &Value, classifier: bool) -> Value {
    match (method, params["method"].as_str()) {
        ("model.find", _) => model(
            params["provider"].as_str().unwrap(),
            params["id"].as_str().unwrap(),
        ),
        ("pi.call", Some("findOfType")) if classifier => {
            json!({ "provider": "box", "id": "qwen", "type": "classifier", "api": "llama-cpp-classify" })
        }
        ("pi.call", Some("classify")) => {
            let prompt = params["args"][1]["state"]["prompt"].as_str().unwrap();
            let complex = if prompt.contains("refactor") {
                0.9
            } else {
                0.1
            };
            let probabilities = json!({ "simple": 1.0 - complex, "complex": complex });
            json!({ "stopReason": "stop", "answers": { "complexity": { "type": "choice", "probabilities": probabilities } } })
        }
        _ => Value::Null,
    }
}

/// The physical model of one route and the methods the extension called.
async fn route(load: fn() -> f64, request: Value, classifier: bool) -> (Value, Vec<String>) {
    let mut host = TestHost::start(extension(Arc::new(Router::new(Some(config()), load))));
    host.initialize().await;
    let params = json!({ "provider": "hybrid", "id": "auto", "request": request, "ctx": ctx() });
    let mut calls = Vec::new();
    let reply = host
        .drive(1, "virtualModel.route", params, |method, params| {
            calls.push(params["method"].as_str().unwrap_or(method).to_string());
            pi(method, params, classifier)
        })
        .await;
    host.finish().await.unwrap();
    (reply["result"].clone(), calls)
}

fn user(text: &str) -> Value {
    json!({ "reason": "user", "thinkingLevel": "medium", "messages": [
        { "role": "user", "content": "earlier" },
        { "role": "user", "content": [{ "type": "text", "text": text }] },
    ] })
}

#[tokio::test]
async fn initialize_declares_hybrid_auto_only_with_a_config() {
    let mut host = TestHost::start(extension(Arc::new(Router::new(Some(config()), idle))));
    let init = host.initialize().await;
    assert_eq!(init["virtualModels"][0]["provider"], "hybrid");
    assert_eq!(init["virtualModels"][0]["id"], "auto");
    assert_eq!(
        init["virtualModels"][0]["thinkingLevels"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
    assert_eq!(init["commands"][0]["name"], "hybrid");
    host.finish().await.unwrap();

    let mut host = TestHost::start(extension(Arc::new(Router::new(None, idle))));
    let init = host.initialize().await;
    assert!(
        init.get("virtualModels")
            .is_none_or(|models| models.as_array().unwrap().is_empty())
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn sends_simple_prompts_to_the_local_model_and_complex_prompts_to_the_remote_model() {
    let (simple, calls) = route(idle, user("what is 2 + 2"), true).await;
    assert_eq!(
        simple,
        json!({ "model": { "provider": "box", "id": "qwen" } })
    );
    assert_eq!(calls, ["model.find", "findOfType", "classify"]);
    let (complex, _) = route(idle, user("refactor the auth layer"), true).await;
    assert_eq!(complex["model"], json!({ "provider": "far", "id": "big" }));
}

#[tokio::test]
async fn skips_the_classifier_while_the_machine_is_busy() {
    let (reply, calls) = route(busy, user("what is 2 + 2"), true).await;
    assert_eq!(reply["model"]["provider"], "far");
    assert_eq!(calls, ["model.find"]);
}

#[tokio::test]
async fn sends_prompts_to_the_remote_model_without_a_classifier() {
    let (reply, calls) = route(idle, user("what is 2 + 2"), false).await;
    assert_eq!(reply["model"]["provider"], "far");
    assert_eq!(calls, ["model.find", "findOfType"]);
}

#[tokio::test]
async fn continues_a_turn_on_the_model_that_answered() {
    let previous = json!({ "model": { "provider": "box", "id": "qwen" }, "thinkingLevel": "low" });
    let request = json!({ "reason": "continuation", "previous": previous, "messages": [] });
    let (reply, calls) = route(idle, request, true).await;
    assert_eq!(
        reply,
        json!({ "model": { "provider": "box", "id": "qwen" }, "thinkingLevel": "low" })
    );
    assert!(calls.is_empty());
}

#[tokio::test]
async fn retries_a_failed_local_request_on_the_remote_model() {
    let failed = json!({ "model": { "provider": "box", "id": "qwen" }, "thinkingLevel": "low" });
    let request = json!({ "reason": "retry", "failed": failed, "messages": [] });
    let (reply, calls) = route(idle, request, true).await;
    assert_eq!(
        reply,
        json!({ "model": { "provider": "far", "id": "big" } })
    );
    assert!(calls.is_empty());
}

#[tokio::test]
async fn setup_saves_the_config_registers_and_selects_hybrid_auto() {
    let dir = temp_dir("setup");
    let router = Arc::new(Router::new(None, idle));
    let mut host = TestHost::start(extension(router.clone())).agent_dir(dir.to_str().unwrap());
    host.initialize().await;
    let mut seen = Vec::new();
    let params = json!({ "name": "hybrid", "args": "", "ctx": ctx() });
    host.drive(1, "command.run", params, |method, params| {
        seen.push(method.to_string());
        match (method, params["method"].as_str()) {
            ("pi.call", Some("getAvailable")) => json!([
                model("box", "qwen"), model("far", "big"), model("far", "small"), model("near", "mid"),
                { "provider": "router", "id": "main", "api": "pi-virtual" }
            ]),
            ("pi.call", Some("getAvailableOfType")) => json!([model("box", "qwen")]),
            ("ui.select", _) => match params["title"].as_str().unwrap() {
                title if title.starts_with("Local model") => {
                    assert_eq!(params["options"], json!(["box/qwen"]));
                    json!("box/qwen")
                }
                "Remote model provider" => {
                    assert_eq!(params["options"], json!(["far", "near"]));
                    json!("far")
                }
                _ => {
                    assert_eq!(params["options"], json!(["big", "small"]));
                    json!("big")
                }
            },
            ("virtualModel.register", _) => {
                assert_eq!(params["provider"], "hybrid");
                Value::Null
            }
            ("model.set", _) => {
                assert_eq!((params["provider"].as_str(), params["id"].as_str()), (Some("hybrid"), Some("auto")));
                json!(true)
            }
            _ => Value::Null,
        }
    })
    .await;
    host.finish().await.unwrap();
    assert!(
        seen.contains(&"virtualModel.register".to_string()),
        "{seen:?}"
    );
    assert_eq!(config::load(&dir).unwrap(), Some(config()));
    assert_eq!(router.config(), Some(config()));
}

#[tokio::test]
async fn off_moves_the_session_to_the_remote_model_and_removes_the_config() {
    let dir = temp_dir("off");
    config::save(&dir, &config()).unwrap();
    let router = Arc::new(Router::new(Some(config()), idle));
    let mut host = TestHost::start(extension(router.clone())).agent_dir(dir.to_str().unwrap());
    host.initialize().await;
    let mut seen = Vec::new();
    let params = json!({ "name": "hybrid", "args": "off", "ctx": ctx() });
    host.drive(1, "command.run", params, |method, params| {
        seen.push(format!(
            "{method} {}",
            params["id"].as_str().unwrap_or_default()
        ));
        match method {
            "model.get" => model("hybrid", "auto"),
            "model.set" => json!(true),
            _ => Value::Null,
        }
    })
    .await;
    host.finish().await.unwrap();
    let order: Vec<&str> = seen
        .iter()
        .map(String::as_str)
        .filter(|call| !call.starts_with("ui."))
        .collect();
    assert_eq!(
        order,
        [
            "model.get ",
            "model.set big",
            "virtualModel.unregister auto"
        ]
    );
    assert_eq!(config::load(&dir).unwrap(), None);
    assert_eq!(router.config(), None);
}

#[test]
fn reads_the_latest_prompt_and_versions() {
    let messages = json!([{ "role": "user", "content": "first" }, { "role": "assistant", "content": [] },
        { "role": "user", "content": [{ "type": "text", "text": "second" }, { "type": "image" }] }]);
    assert_eq!(latest_prompt(&messages), "second");
    assert_eq!(parse_version("0.12.12"), Some((0, 12, 12)));
    assert_eq!(parse_version("v0.13.0-rc.1"), Some((0, 13, 0)));
    assert!(parse_version("0.9.2") < Some((0, 12, 12)));
    let saved = serde_json::to_value(config()).unwrap();
    assert_eq!(saved, json!({ "local": "box/qwen", "remote": "far/big" }));
}
