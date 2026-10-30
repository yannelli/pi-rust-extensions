mod common;

use common::{MockServer, Reply};
use pi_ext_web_search::api::{self, Endpoint};
use pi_ext_web_search::config::Api;
use pi_ext_web_search::http::Call;
use serde_json::json;

fn endpoint(api: Api, server: &MockServer) -> Endpoint {
    Endpoint {
        api,
        base_url: server.url.clone(),
        key: "k-test".into(),
        max_results: 5,
    }
}

fn urls(list: &[&str]) -> Vec<String> {
    list.iter().map(|u| u.to_string()).collect()
}

#[tokio::test]
async fn exa_search_and_contents() {
    let search = json!({ "results": [
        { "title": "Rust", "url": "https://rust-lang.org/", "publishedDate": "2026-01-01", "highlights": ["fast", "safe"] }
    ] });
    let contents = json!({
        "results": [{ "url": "https://a.test/", "title": "A", "text": "page a" }],
        "statuses": [{ "id": "https://a.test/", "status": "success" }, { "id": "https://b.test/", "status": "error", "error": { "tag": "CRAWL_NOT_FOUND" } }]
    });
    let server = MockServer::start(vec![
        ("/search", Reply::json(search)),
        ("/contents", Reply::json(contents)),
    ])
    .await;
    let result = api::web_search(&Call::quiet(), &endpoint(Api::Exa, &server), "rust", &[])
        .await
        .unwrap();
    let request = server.only();
    assert_eq!(
        (request.method.as_str(), request.path.as_str()),
        ("POST", "/search")
    );
    assert_eq!(request.headers["x-api-key"], "k-test");
    assert_eq!(
        request.body,
        json!({ "query": "rust", "numResults": 5, "type": "auto", "contents": { "highlights": true } })
    );
    assert!(result.text.contains(
        "1. Rust\n   https://rust-lang.org/\n   Published: 2026-01-01\n   fast ... safe"
    ));
    assert_eq!(result.sources[0].url, "https://rust-lang.org/");

    let pages = api::url_context(
        &Call::quiet(),
        &endpoint(Api::Exa, &server),
        "q",
        &urls(&["https://a.test/", "https://b.test/"]),
    )
    .await
    .unwrap();
    assert_eq!(
        server.requests()[1].body,
        json!({ "urls": ["https://a.test/", "https://b.test/"], "text": true })
    );
    assert!(pages.text.contains("## A\nhttps://a.test/\n\npage a"));
    assert!(
        pages
            .text
            .contains("## Failed: https://b.test/\nCRAWL_NOT_FOUND")
    );
}

#[tokio::test]
async fn firecrawl_search_and_scrape() {
    let search = json!({ "success": true, "data": { "web": [{ "url": "https://f.test/", "title": "F", "description": "desc" }] } });
    let scrape = json!({ "success": true, "data": { "markdown": "# Page", "metadata": { "title": "Page", "statusCode": 200 } } });
    let server = MockServer::start(vec![
        ("/v2/search", Reply::json(search)),
        ("/v2/scrape", Reply::json(scrape)),
    ])
    .await;
    let ep = endpoint(Api::Firecrawl, &server);
    let result = api::web_search(&Call::quiet(), &ep, "q", &urls(&["https://p.test/"]))
        .await
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests[0].headers["authorization"], "Bearer k-test");
    assert_eq!(
        requests[0].body,
        json!({ "query": "q", "limit": 5, "sources": ["web"] })
    );
    assert_eq!(
        requests[1].body,
        json!({ "url": "https://p.test/", "formats": ["markdown"], "onlyMainContent": true })
    );
    assert!(result.text.contains("1. F\n   https://f.test/\n   desc"));
    assert!(
        result
            .text
            .contains("## Fetched: Page\nhttps://p.test/\n\n# Page")
    );
}

#[tokio::test]
async fn firecrawl_reports_failure_payload() {
    let server = MockServer::start(vec![(
        "/v2/search",
        Reply::json(json!({ "success": false, "error": "quota" })),
    )])
    .await;
    let err = api::web_search(&Call::quiet(), &endpoint(Api::Firecrawl, &server), "q", &[])
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "Firecrawl error: quota");
}

#[tokio::test]
async fn brave_search_uses_query_and_token() {
    let body = json!({ "web": { "results": [{ "title": "B", "url": "https://b.test/", "description": "d", "extra_snippets": ["e"] }] } });
    let server = MockServer::start(vec![("/res/v1/web/search", Reply::json(body))]).await;
    let result = api::web_search(&Call::quiet(), &endpoint(Api::Brave, &server), "a b", &[])
        .await
        .unwrap();
    let request = server.only();
    assert_eq!(
        (request.method.as_str(), request.path.as_str()),
        ("GET", "/res/v1/web/search?q=a+b&count=5")
    );
    assert_eq!(request.headers["x-subscription-token"], "k-test");
    assert!(result.text.contains("1. B\n   https://b.test/\n   d ... e"));
    let err = api::url_context(
        &Call::quiet(),
        &endpoint(Api::Brave, &server),
        "q",
        &urls(&["https://x.test/"]),
    )
    .await
    .unwrap_err();
    assert_eq!(err.to_string(), "Brave Search has no page fetch API");
}

#[tokio::test]
async fn tavily_search_and_extract() {
    let search = json!({ "answer": "Short answer", "results": [{ "title": "T", "url": "https://t.test/", "content": "c", "score": 0.9 }] });
    let extract = json!({ "results": [{ "url": "https://t.test/", "raw_content": "raw" }], "failed_results": [{ "url": "https://bad.test/", "error": "blocked" }] });
    let server = MockServer::start(vec![
        ("/search", Reply::json(search)),
        ("/extract", Reply::json(extract)),
    ])
    .await;
    let ep = endpoint(Api::Tavily, &server);
    let result = api::web_search(&Call::quiet(), &ep, "q", &[])
        .await
        .unwrap();
    let request = server.only();
    assert_eq!(request.headers["authorization"], "Bearer k-test");
    assert_eq!(
        request.body,
        json!({ "query": "q", "max_results": 5, "search_depth": "basic", "include_answer": true })
    );
    assert!(
        result
            .text
            .starts_with("Search results for \"q\" via Tavily:\n\nAnswer: Short answer")
    );
    let pages = api::url_context(
        &Call::quiet(),
        &ep,
        "q",
        &urls(&["https://t.test/", "https://bad.test/"]),
    )
    .await
    .unwrap();
    assert_eq!(
        server.requests()[1].body,
        json!({ "urls": ["https://t.test/", "https://bad.test/"], "format": "markdown" })
    );
    assert!(
        pages.text.contains("raw") && pages.text.contains("## Failed: https://bad.test/\nblocked")
    );
}

#[tokio::test]
async fn http_errors_name_the_provider() {
    let server = MockServer::start(vec![("/search", Reply::status(401, "bad key"))]).await;
    let err = api::web_search(&Call::quiet(), &endpoint(Api::Exa, &server), "q", &[])
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "Exa API error (401): bad key");
}
