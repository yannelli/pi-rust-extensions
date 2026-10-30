//! Ollama Cloud `POST /api/web_search` and `/api/web_fetch`.
//! Docs: https://docs.ollama.com/capabilities/web-search

use anyhow::{Result, anyhow};
use serde::Deserialize;
use serde_json::json;

use crate::http::{ApiError, Call};
use crate::model::Target;
use crate::pages::{Hit, Page, add_pages, search_result, url_context_result};
use crate::results::StreamResult;

const MAX_RESULTS: u32 = 10;
const KEY_HINT: &str = "set OLLAMA_API_KEY or run /login ollama-cloud.";

/// Strips the OpenAI-compatible `/vN` suffix so `/api/*` resolves.
pub fn api_root(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    match base.rsplit_once('/') {
        Some((head, tail))
            if tail.len() > 1
                && tail[..1].eq_ignore_ascii_case("v")
                && tail[1..].bytes().all(|b| b.is_ascii_digit()) =>
        {
            head.to_owned()
        }
        _ => base.to_owned(),
    }
}

fn key(target: &Target) -> Result<String> {
    let key = target.auth.api_key.clone().or_else(|| {
        std::env::var("OLLAMA_API_KEY")
            .ok()
            .filter(|k| !k.is_empty())
    });
    key.ok_or_else(|| anyhow!("Ollama web search needs an API key: {KEY_HINT}"))
}

async fn post<T: for<'de> Deserialize<'de>>(
    call: &Call,
    target: &Target,
    path: &str,
    body: serde_json::Value,
) -> Result<T> {
    let url = format!("{}{path}", api_root(&target.model.base_url));
    let request = call
        .client
        .post(url)
        .bearer_auth(key(target)?)
        .header("accept", "application/json")
        .json(&body);
    call.json(request, "Ollama")
        .await
        .map_err(|err| match err.downcast_ref::<ApiError>() {
            Some(api) if api.status == 401 => {
                anyhow!("Ollama API key missing or invalid: {KEY_HINT}")
            }
            _ => err,
        })
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchResponse {
    results: Vec<SearchHit>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchHit {
    title: Option<String>,
    url: Option<String>,
    content: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct FetchResponse {
    title: Option<String>,
    content: Option<String>,
}

async fn fetch_pages(call: &Call, target: &Target, urls: &[String]) -> Vec<Page> {
    let mut pages = Vec::new();
    for url in urls {
        let page = match post::<FetchResponse>(
            call,
            target,
            "/api/web_fetch",
            json!({ "url": url }),
        )
        .await
        {
            Ok(data) => Page {
                url: url.clone(),
                title: data.title,
                content: data.content,
                error: None,
            },
            Err(err) => Page::failed(url, err.to_string()),
        };
        pages.push(page);
    }
    pages
}

pub async fn search(
    call: &Call,
    target: &Target,
    query: &str,
    urls: &[String],
) -> Result<StreamResult> {
    key(target)?;
    call.progress("Searching the web with Ollama...");
    let body = json!({ "query": query, "max_results": MAX_RESULTS });
    let data: SearchResponse = post(call, target, "/api/web_search", body).await?;
    let hits: Vec<Hit> = data
        .results
        .into_iter()
        .filter_map(|hit| {
            Some(Hit {
                url: hit.url?,
                title: hit.title,
                content: hit.content,
                published: None,
            })
        })
        .collect();
    let mut result = search_result(query, "Ollama", "ollama", &hits, None);
    if !urls.is_empty() {
        call.progress(&format!("Fetching {} URL(s) with Ollama...", urls.len()));
        add_pages(&mut result, &fetch_pages(call, target, urls).await);
    }
    Ok(result)
}

pub async fn url_context(
    call: &Call,
    target: &Target,
    query: &str,
    urls: &[String],
) -> Result<StreamResult> {
    key(target)?;
    let pages = fetch_pages(call, target, urls).await;
    Ok(url_context_result(query, "ollama", &pages))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_root_strips_version_suffix() {
        assert_eq!(api_root("https://ollama.com/v1/"), "https://ollama.com");
        assert_eq!(api_root("https://ollama.com/api"), "https://ollama.com/api");
        assert_eq!(api_root("https://ollama.com"), "https://ollama.com");
    }
}
