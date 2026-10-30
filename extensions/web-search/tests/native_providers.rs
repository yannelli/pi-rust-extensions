mod common;

use common::{MockServer, Reply};
use pi_ext_web_search::http::Call;
use pi_ext_web_search::model::{Auth, SearchModel, Target};
use pi_ext_web_search::native;
use serde_json::{Value, json};

fn target(raw: Value, key: &str) -> Target {
    Target {
        model: SearchModel::from_json(raw).unwrap(),
        auth: Auth {
            api_key: Some(key.into()),
            ..Auth::default()
        },
        session_id: Some("s1".into()),
        thinking_level: Some("high".into()),
    }
}

#[tokio::test]
async fn openai_responses_request_and_citations() {
    let events = [
        json!({ "type": "response.web_search_call.searching", "item_id": "ws1" }),
        json!({ "type": "response.output_item.done", "item": { "type": "web_search_call", "id": "ws1", "status": "completed",
            "action": { "type": "search", "query": "rust 2026", "sources": [{ "url": "https://r.test/a?utm_source=x", "title": "R" }] } } }),
        json!({ "type": "response.output_text.delta", "delta": "Rust is fast." }),
        json!({ "type": "response.output_text.annotation.added", "annotation": { "type": "url_citation", "url": "https://r.test/a", "title": "R", "end_index": 13 } }),
        json!({ "type": "response.completed", "response": { "output": [] } }),
    ];
    let server = MockServer::start(vec![("/v1/responses", Reply::sse(&events))]).await;
    let raw = json!({ "id": "gpt-5.1", "provider": "opencode", "api": "openai-responses", "baseUrl": format!("{}/v1", server.url),
        "reasoning": true, "headers": { "x-extra": "1" } });
    let result = native::web_search(
        &Call::quiet(),
        &target(raw, "sk-1"),
        "rust",
        &["https://u.test/".into()],
    )
    .await
    .unwrap();
    let request = server.only();
    assert_eq!(request.headers["authorization"], "Bearer sk-1");
    assert_eq!(request.headers["x-opencode-session"], "s1");
    assert_eq!(request.headers["x-extra"], "1");
    assert_eq!(request.body["tools"], json!([{ "type": "web_search" }]));
    assert_eq!(
        request.body["include"],
        json!(["web_search_call.action.sources", "web_search_call.results"])
    );
    assert_eq!(request.body["reasoning"], json!({ "effort": "high" }));
    assert_eq!(
        request.body["input"][0]["content"][0]["text"],
        "rust\n\nAlso analyze these URLs:\nhttps://u.test/"
    );
    assert_eq!(result.text, "Rust is fast.[1]");
    assert_eq!(result.search_queries, ["rust 2026"]);
    assert_eq!(result.sources[0].url, "https://r.test/a");
    assert_eq!(
        result.native_search_calls[0].status.as_deref(),
        Some("completed")
    );
}

#[tokio::test]
async fn xai_uses_grok_shape_without_reasoning() {
    let events = [
        json!({ "type": "response.output_text.delta", "delta": "ok" }),
        json!({ "type": "response.completed", "response": {} }),
    ];
    let server = MockServer::start(vec![("/responses", Reply::sse(&events))]).await;
    let raw = json!({ "id": "grok-4.6", "provider": "xai", "api": "openai-responses", "baseUrl": server.url, "reasoning": true });
    let result = native::web_search(&Call::quiet(), &target(raw, "xk"), "q", &[])
        .await
        .unwrap();
    let body = server.only().body;
    assert_eq!(body["input"], json!([{ "role": "user", "content": "q" }]));
    assert_eq!(body["include"], json!(["web_search_call.action.sources"]));
    assert!(body.get("reasoning").is_none());
    assert_eq!(result.provider_kind, "xai");
}

#[tokio::test]
async fn anthropic_messages_tool_and_text_citations() {
    let events = [
        json!({ "type": "content_block_start", "content_block": { "type": "server_tool_use", "id": "t1", "name": "web_search", "input": { "query": "news" } } }),
        json!({ "type": "content_block_start", "content_block": { "type": "web_search_tool_result", "tool_use_id": "t1",
            "content": [{ "type": "web_search_result", "url": "https://n.test/", "title": "N", "page_age": "1 day" }] } }),
        json!({ "type": "content_block_delta", "delta": { "type": "text_delta", "text": "Big news today." } }),
        json!({ "type": "content_block_delta", "delta": { "type": "citations_delta", "citation": {
            "type": "web_search_result_location", "url": "https://n.test/", "title": "N", "cited_text": "Big news" } } }),
    ];
    let server = MockServer::start(vec![("/v1/messages", Reply::sse(&events))]).await;
    let raw = json!({ "id": "claude-opus-5-5", "provider": "anthropic", "api": "anthropic-messages", "baseUrl": server.url, "maxTokens": 64000 });
    let result = native::web_search(&Call::quiet(), &target(raw, "sk-ant-api"), "news", &[])
        .await
        .unwrap();
    let request = server.only();
    assert_eq!(request.headers["x-api-key"], "sk-ant-api");
    assert_eq!(request.headers["anthropic-version"], "2023-06-01");
    assert_eq!(request.body["max_tokens"], 8192);
    assert_eq!(request.body["tools"][0]["type"], "web_search_20260209");
    assert_eq!(result.text, "Big news[1] today.");
    assert_eq!(result.search_queries, ["news"]);
    assert_eq!(result.search_results[0].page_age.as_deref(), Some("1 day"));
}

#[tokio::test]
async fn anthropic_oauth_adds_claude_code_headers() {
    let server = MockServer::start(vec![("/v1/messages", Reply::sse(&[]))]).await;
    let raw = json!({ "id": "c", "provider": "anthropic", "api": "anthropic-messages", "baseUrl": server.url });
    let result = native::web_search(&Call::quiet(), &target(raw, "sk-ant-oat-1"), "q", &[])
        .await
        .unwrap();
    let request = server.only();
    assert_eq!(request.headers["authorization"], "Bearer sk-ant-oat-1");
    assert_eq!(
        request.headers["anthropic-beta"],
        "claude-code-20250219,oauth-2025-04-20"
    );
    assert_eq!(
        request.body["system"][0]["text"],
        "You are Claude Code, Anthropic's official CLI for Claude."
    );
    assert_eq!(result.text, "No answer available.");
}

#[tokio::test]
async fn deepseek_routes_to_anthropic_path_and_fails_on_search_error() {
    let events = [
        json!({ "type": "content_block_start", "content_block": { "type": "web_search_tool_result", "tool_use_id": "t",
        "content": { "type": "web_search_tool_result_error", "error_code": "max_uses_exceeded" } } }),
    ];
    let server = MockServer::start(vec![("/anthropic/v1/messages", Reply::sse(&events))]).await;
    let raw = json!({ "id": "deepseek-v4", "provider": "deepseek", "api": "openai-completions", "baseUrl": format!("{}/v1", server.url) });
    let err = native::web_search(&Call::quiet(), &target(raw, "dk"), "q", &[])
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "DeepSeek web search failed: max_uses_exceeded"
    );
    assert_eq!(server.only().path, "/anthropic/v1/messages");
}

#[tokio::test]
async fn gemini_grounding_and_url_context() {
    let events = [
        json!({ "candidates": [{ "content": { "parts": [{ "text": "Hello world" }] } }] }),
        json!({ "candidates": [{ "groundingMetadata": {
            "webSearchQueries": ["hello"],
            "groundingChunks": [{ "web": { "uri": "https://g.test/", "title": "G" } }],
            "groundingSupports": [{ "segment": { "endIndex": 5, "text": "Hello" }, "groundingChunkIndices": [0] }]
        }, "urlContextMetadata": { "urlMetadata": [{ "retrievedUrl": "https://u.test/", "urlRetrievalStatus": "URL_RETRIEVAL_STATUS_SUCCESS" }] } }] }),
    ];
    let server = MockServer::start(vec![("/v1beta/models/", Reply::sse(&events))]).await;
    let raw = json!({ "id": "gemini-3.7-flash", "provider": "google", "api": "google-generative-ai", "baseUrl": format!("{}/v1beta", server.url) });
    let t = target(raw, "gk");
    let result = native::web_search(&Call::quiet(), &t, "hi", &["https://u.test/".into()])
        .await
        .unwrap();
    let request = server.only();
    assert_eq!(
        request.path,
        "/v1beta/models/gemini-3.7-flash:streamGenerateContent?alt=sse"
    );
    assert_eq!(request.headers["x-goog-api-key"], "gk");
    assert_eq!(
        request.body["tools"],
        json!([{ "google_search": {} }, { "url_context": {} }])
    );
    assert_eq!(result.search_queries, ["hello"]);
    let formatted = pi_ext_web_search::format::format_web_search(&result, "gemini");
    assert!(
        formatted.content[0]["text"]
            .as_str()
            .unwrap()
            .starts_with("Hello[1] world")
    );

    native::url_context(
        &Call::quiet(),
        &t,
        "sum",
        &["https://youtu.be/dQw4w9WgXcQ".into()],
    )
    .await
    .unwrap();
    let body = &server.requests()[1].body;
    assert_eq!(
        body["contents"][0]["parts"][0]["file_data"]["mime_type"],
        "video/mp4"
    );
    assert_eq!(body["tools"], json!([{ "url_context": {} }]));
}

#[tokio::test]
async fn gemini_cli_wraps_cloud_code_envelope() {
    let events = [
        json!({ "response": { "candidates": [{ "content": { "parts": [{ "text": "wrapped" }] } }] } }),
    ];
    let server = MockServer::start(vec![(
        "/v1internal:streamGenerateContent",
        Reply::sse(&events),
    )])
    .await;
    let raw = json!({ "id": "gemini-3.7-pro", "provider": "google-gemini-cli", "api": "google-gemini-cli", "baseUrl": server.url });
    let result = native::web_search(
        &Call::quiet(),
        &target(raw, r#"{"token":"tok","projectId":"proj"}"#),
        "q",
        &[],
    )
    .await
    .unwrap();
    let request = server.only();
    assert_eq!(request.headers["authorization"], "Bearer tok");
    assert_eq!(request.body["project"], "proj");
    assert_eq!(
        request.body["request"]["tools"],
        json!([{ "google_search": {} }])
    );
    assert_eq!(result.text, "wrapped");
}

#[tokio::test]
async fn ollama_search_and_fetch() {
    let search =
        json!({ "results": [{ "title": "O", "url": "https://o.test/", "content": "snippet" }] });
    let fetch = json!({ "title": "Page", "content": "body", "links": [] });
    let server = MockServer::start(vec![
        ("/api/web_search", Reply::json(search)),
        ("/api/web_fetch", Reply::json(fetch)),
    ])
    .await;
    let raw = json!({ "id": "gpt-oss", "provider": "ollama-cloud", "api": "openai-completions", "baseUrl": format!("{}/v1", server.url) });
    let t = target(raw, "ok");
    let result = native::web_search(&Call::quiet(), &t, "q", &["https://p.test/".into()])
        .await
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests[0].body, json!({ "query": "q", "max_results": 10 }));
    assert_eq!(requests[0].headers["authorization"], "Bearer ok");
    assert_eq!(requests[1].body, json!({ "url": "https://p.test/" }));
    assert!(
        result
            .text
            .starts_with("Search results for \"q\" via Ollama:")
    );
    assert!(
        result
            .text
            .contains("## Fetched: Page\nhttps://p.test/\n\nbody")
    );
    let pages = native::url_context(&Call::quiet(), &t, "q", &["https://p.test/".into()])
        .await
        .unwrap();
    assert_eq!(pages.provider_kind, "ollama");
}
