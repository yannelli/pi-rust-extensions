mod common;

use common::host::{TestHost, ctx};
use common::{MockServer, Reply, TempDir};
use pi_ext_web_search::extension;
use serde_json::{Value, json};

fn exa_reply() -> Reply {
    Reply::json(
        json!({ "results": [{ "title": "Hit", "url": "https://hit.test/", "highlights": ["h"] }] }),
    )
}

async fn started(agent: &TempDir, cwd: &TempDir) -> TestHost {
    let mut host = TestHost::start(extension());
    host.initialize(agent.0.to_str().unwrap(), cwd.0.to_str().unwrap())
        .await;
    host
}

fn execute(name: &str, params: Value, cwd: &TempDir) -> Value {
    json!({ "toolCallId": "c1", "name": name, "params": params, "ctx": ctx(cwd.0.to_str().unwrap()) })
}

#[tokio::test]
async fn initialize_registers_tools_command_and_events() {
    let mut host = TestHost::start(extension());
    let result = host.initialize("/tmp/agent", "/tmp/work").await;
    let names: Vec<&str> = result["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["web_search", "url_context"]);
    assert_eq!(result["tools"][0]["label"], "Web Search");
    assert_eq!(
        result["tools"][1]["parameters"]["required"],
        json!(["query", "urls"])
    );
    assert_eq!(result["commands"][0]["name"], "web-search");
    assert_eq!(
        result["events"],
        json!(["session_start", "session_tree", "model_select"])
    );
    host.finish().await;
}

#[tokio::test]
async fn web_search_uses_global_exa_config_and_ignores_untrusted_project() {
    let (agent, cwd) = (TempDir::new(), TempDir::new());
    let good = MockServer::start(vec![("/search", exa_reply())]).await;
    let evil = MockServer::start(vec![("/search", exa_reply())]).await;
    agent.write(
        "ext/web-search.json",
        &json!({ "defaultProvider": "exa", "providers": { "exa": { "baseUrl": good.url } } }),
    );
    agent.write("ext/web-search/keys.json", &json!({ "exa": "stored-key" }));
    cwd.write(
        ".pi/ext/web-search.json",
        &json!({ "providers": { "exa": { "baseUrl": evil.url } } }),
    );
    let mut host = started(&agent, &cwd).await;
    host.request(
        1,
        "tool.execute",
        execute("web_search", json!({ "query": "rust" }), &cwd),
    )
    .await;
    let response = host.expect_response(1).await;
    let result = &response["result"];
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("1. Hit\n   https://hit.test/")
    );
    assert_eq!(result["details"]["model"], "exa");
    assert_eq!(result["details"]["providerKind"], "exa");
    assert_eq!(good.only().headers["x-api-key"], "stored-key");
    assert!(evil.requests().is_empty());
    host.finish().await;
}

#[tokio::test]
async fn native_choice_reports_unsupported_model() {
    let (agent, cwd) = (TempDir::new(), TempDir::new());
    agent.write(
        "ext/web-search.json",
        &json!({ "defaultProvider": "native" }),
    );
    let mut host = started(&agent, &cwd).await;
    host.request(
        1,
        "tool.execute",
        execute("web_search", json!({ "query": "q" }), &cwd),
    )
    .await;
    let model = json!({ "id": "llama", "provider": "groq", "api": "openai-completions", "baseUrl": "http://127.0.0.1:9" });
    host.answer_pi_call("ctx", "model", model).await;
    host.answer_pi_call("ctx.modelRegistry", "getAvailable", json!([]))
        .await;
    let result = host.expect_response(1).await["result"].clone();
    assert_eq!(result["details"]["error"], "unsupported_model");
    assert_eq!(result["isError"], true);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("Failed: The current model llama (groq/openai-completions) does not support native web search."));
    host.finish().await;
}

#[tokio::test]
async fn auto_uses_current_model_credentials() {
    let (agent, cwd) = (TempDir::new(), TempDir::new());
    let events = [
        json!({ "type": "response.output_text.delta", "delta": "native answer" }),
        json!({ "type": "response.completed", "response": {} }),
    ];
    let server = MockServer::start(vec![("/responses", Reply::sse(&events))]).await;
    let mut host = started(&agent, &cwd).await;
    host.request(
        1,
        "tool.execute",
        execute("web_search", json!({ "query": "q" }), &cwd),
    )
    .await;
    let model = json!({ "id": "gpt-5.1", "provider": "openai", "api": "openai-responses", "baseUrl": server.url });
    host.answer_pi_call("ctx", "model", model.clone()).await;
    let params = host
        .answer_pi_call(
            "ctx.modelRegistry",
            "getApiKeyAndHeaders",
            json!({ "ok": true, "apiKey": "sk-x" }),
        )
        .await;
    assert_eq!(params["args"][0], model);
    let result = host.expect_response(1).await["result"].clone();
    assert_eq!(result["content"][0]["text"], "native answer");
    assert_eq!(result["details"]["model"], "gpt-5.1");
    assert_eq!(server.only().headers["authorization"], "Bearer sk-x");
    host.finish().await;
}

#[tokio::test]
async fn url_context_with_brave_is_unsupported() {
    let (agent, cwd) = (TempDir::new(), TempDir::new());
    agent.write(
        "ext/web-search.json",
        &json!({ "tools": { "url_context": "brave" } }),
    );
    let mut host = started(&agent, &cwd).await;
    host.request(
        1,
        "tool.execute",
        execute(
            "url_context",
            json!({ "query": "q", "urls": ["https://a.test/"] }),
            &cwd,
        ),
    )
    .await;
    let result = host.expect_response(1).await["result"].clone();
    assert_eq!(result["details"]["error"], "unsupported_provider");
    assert_eq!(result["details"]["providerKind"], "brave");
    host.finish().await;
}

#[tokio::test]
async fn session_start_hides_url_context_without_a_provider() {
    let (agent, cwd) = (TempDir::new(), TempDir::new());
    agent.write(
        "ext/web-search.json",
        &json!({ "tools": { "url_context": "native" } }),
    );
    let mut host = started(&agent, &cwd).await;
    let event = json!({ "name": "session_start", "event": { "type": "session_start" }, "ctx": ctx(cwd.0.to_str().unwrap()) });
    host.request(1, "event", event).await;
    let model = json!({ "id": "gpt", "provider": "openai", "api": "openai-responses", "baseUrl": "https://api.openai.com/v1" });
    host.answer_pi_call("ctx", "model", model).await;
    let (id, _) = host.expect_request("tools.getActive").await;
    host.answer(&id, json!(["read", "web_search", "url_context"]))
        .await;
    let (id, params) = host.expect_request("tools.setActive").await;
    assert_eq!(params["names"], json!(["read", "web_search"]));
    host.answer(&id, Value::Null).await;
    host.expect_response(1).await;
    host.finish().await;
}

#[tokio::test]
async fn command_stores_api_key_with_private_mode() {
    let (agent, cwd) = (TempDir::new(), TempDir::new());
    let mut host = started(&agent, &cwd).await;
    let params = json!({ "name": "web-search", "args": "", "ctx": ctx(cwd.0.to_str().unwrap()) });
    host.request(1, "command.run", params).await;
    let (id, menu) = host.expect_request("ui.select").await;
    assert_eq!(menu["options"][0], "Default provider: auto");
    host.answer(&id, json!("API keys")).await;
    let (id, keys) = host.expect_request("ui.select").await;
    let exa = keys["options"][0].as_str().unwrap().to_owned();
    assert!(exa.starts_with("Exa: "), "{exa}");
    host.answer(&id, json!(exa)).await;
    let (id, _) = host.expect_request("ui.select").await;
    host.answer(&id, json!("Set key")).await;
    let (id, _) = host.expect_request("ui.input").await;
    host.answer(&id, json!("exa-secret-123")).await;
    let (id, note) = host.expect_request("ui.notify").await;
    assert!(
        note["message"]
            .as_str()
            .unwrap()
            .starts_with("Stored Exa key")
    );
    host.answer(&id, Value::Null).await;
    let (id, menu) = host.expect_request("ui.select").await;
    assert_eq!(menu["title"], "Web search");
    host.answer(&id, Value::Null).await;
    host.answer_pi_call("ctx", "model", Value::Null).await;
    let (id, _) = host.expect_request("tools.getActive").await;
    host.answer(&id, json!(["web_search", "url_context"])).await;
    host.expect_response(1).await;
    let path = agent.0.join("ext/web-search/keys.json");
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(stored, json!({ "exa": "exa-secret-123" }));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    host.finish().await;
}
