mod common;
mod mock;

use common::TestHost;
use mock::MockOllama;
use pi_ext_ollama::client::Client;
use pi_ext_ollama::discovery::discover;
use pi_ext_ollama::extension;
use serde_json::{Value, json};

fn ctx() -> Value {
    json!({ "cwd": "/nonexistent-project", "hasUI": true, "flags": {}, "isProjectTrusted": false })
}

#[derive(Default)]
struct Seen {
    statuses: Vec<String>,
    notes: Vec<(String, String)>,
    registered: Vec<Value>,
    responses: Vec<u64>,
}

/// Answers host requests and records them until `done` holds.
async fn pump(host: &mut TestHost, seen: &mut Seen, done: impl Fn(&Seen) -> bool) {
    while !done(seen) {
        let message = host.recv().await;
        let id = message["id"].clone();
        let params = message["params"].clone();
        match message["method"].as_str() {
            Some("provider.register") => seen.registered.push(params),
            Some("ui.setStatus") => {
                if let Some(text) = params["text"].as_str() {
                    seen.statuses.push(text.to_string());
                }
            }
            Some("ui.notify") => seen.notes.push((
                params["type"].as_str().unwrap_or("info").to_string(),
                params["message"].as_str().unwrap_or("").to_string(),
            )),
            Some(other) => panic!("unexpected request {other}: {message}"),
            None => {
                seen.responses.push(id.as_u64().unwrap());
                continue;
            }
        }
        if !id.is_null() {
            host.answer(&id, Value::Null).await;
        }
    }
}

#[tokio::test]
async fn session_start_discovers_models_and_registers_the_provider() {
    let mock = MockOllama::start(&["qwen3:8b", "nomic-embed-text"]);
    let mut host = TestHost::start(extension(Some(mock.url().to_string()), None));
    let init = host.initialize().await;
    assert!(
        init.get("providers")
            .is_none_or(|p| p.as_array().unwrap().is_empty())
    );
    host.request(
        1,
        "event",
        json!({ "name": "session_start", "event": {"type": "session_start"}, "ctx": ctx() }),
    )
    .await;
    let mut seen = Seen::default();
    pump(&mut host, &mut seen, |s| {
        s.responses.contains(&1) && !s.registered.is_empty()
    })
    .await;
    let params = &seen.registered[0];
    assert_eq!(params["name"], "ollama");
    let config = &params["config"];
    assert_eq!(config["baseUrl"], format!("{}/v1", mock.url()));
    assert_eq!(config["api"], "openai-completions");
    assert_eq!(config["apiKey"], "ollama");
    let models = config["models"].as_array().unwrap();
    assert_eq!(
        models.len(),
        1,
        "embedding models are not chat models: {models:?}"
    );
    assert_eq!(models[0]["id"], "qwen3:8b");
    assert_eq!(models[0]["contextWindow"], 40960);
    assert_eq!(models[0]["reasoning"], true);
    let paths: Vec<String> = mock
        .requests()
        .into_iter()
        .map(|(_, path, _)| path)
        .collect();
    assert_eq!(paths, ["/api/tags", "/api/show", "/api/show"]);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn pull_reports_progress_then_registers_the_new_model() {
    let mock = MockOllama::start(&["qwen3:8b"]);
    let mut host = TestHost::start(extension(Some(mock.url().to_string()), None));
    host.initialize().await;
    let params = json!({ "name": "ollama", "args": "pull qwen3:14b", "ctx": ctx() });
    host.request(2, "command.run", params).await;
    let mut seen = Seen::default();
    pump(&mut host, &mut seen, |s| s.responses.contains(&2)).await;
    assert!(
        seen.statuses
            .iter()
            .any(|s| s.contains("25% (1.0 KB / 4.0 KB)")),
        "{:?}",
        seen.statuses
    );
    assert!(
        seen.statuses.iter().any(|s| s.contains("100%")),
        "{:?}",
        seen.statuses
    );
    assert!(
        seen.statuses.iter().any(|s| s.ends_with("success")),
        "{:?}",
        seen.statuses
    );
    let models = seen.registered.last().unwrap()["config"]["models"]
        .as_array()
        .unwrap()
        .clone();
    let ids: Vec<&str> = models.iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["qwen3:14b", "qwen3:8b"]);
    assert!(
        seen.notes
            .last()
            .unwrap()
            .1
            .starts_with("Pulled qwen3:14b. Registered 2 Ollama models")
    );

    host.request(
        3,
        "command.run",
        json!({ "name": "ollama", "args": "pull missing:1", "ctx": ctx() }),
    )
    .await;
    let mut failed = Seen::default();
    pump(&mut host, &mut failed, |s| s.responses.contains(&3)).await;
    let (kind, text) = failed.notes.last().unwrap();
    assert_eq!(kind, "error");
    assert!(text.contains("file does not exist"), "{text}");
    host.finish().await.unwrap();
}

#[tokio::test]
async fn startup_discovery_declares_the_provider_in_initialize() {
    let mock = MockOllama::start(&["qwen3:8b"]);
    let models = discover(&Client::new(mock.url())).await.unwrap();
    let mut host = TestHost::start(extension(None, Some((mock.url().to_string(), models))));
    let init = host.initialize().await;
    let provider = &init["providers"][0];
    assert_eq!(provider["name"], "ollama");
    assert_eq!(provider["config"]["models"][0]["id"], "qwen3:8b");
    host.finish().await.unwrap();
}

#[tokio::test]
async fn delete_removes_the_model() {
    let mock = MockOllama::start(&["qwen3:8b", "llama3:8b"]);
    let client = Client::new(mock.url());
    client.delete("llama3:8b").await.unwrap();
    assert_eq!(mock.models(), ["qwen3:8b"]);
    let (method, _, body) = mock.requests().pop().unwrap();
    assert_eq!(
        (method.as_str(), body),
        ("DELETE", json!({"model": "llama3:8b"}))
    );
}
