//! Exa `POST /search` and `POST /contents`.
//! Docs: https://exa.ai/docs/reference/search, https://exa.ai/docs/reference/get-contents

use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

use super::{Endpoint, Found, join_snippets};
use crate::http::Call;
use crate::pages::{Hit, Page};

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ExaResult {
    title: Option<String>,
    url: Option<String>,
    id: Option<String>,
    published_date: Option<String>,
    text: Option<String>,
    highlights: Vec<String>,
    summary: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ExaStatus {
    id: String,
    status: String,
    error: Option<serde_json::Value>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ExaResponse {
    results: Vec<ExaResult>,
    statuses: Vec<ExaStatus>,
}

pub async fn search(call: &Call, endpoint: &Endpoint, query: &str) -> Result<Found> {
    let body = json!({
        "query": query, "numResults": endpoint.max_results, "type": "auto",
        "contents": { "highlights": true }
    });
    let request = call
        .client
        .post(format!("{}/search", endpoint.base_url))
        .header("x-api-key", &endpoint.key)
        .json(&body);
    let data: ExaResponse = call.json(request, "Exa").await?;
    let hits = data
        .results
        .into_iter()
        .filter_map(|r| {
            let content = join_snippets(&r.highlights).or(r.summary).or(r.text);
            Some(Hit {
                url: r.url?,
                title: r.title,
                content,
                published: r.published_date,
            })
        })
        .collect();
    Ok(Found { hits, answer: None })
}

pub async fn contents(call: &Call, endpoint: &Endpoint, urls: &[String]) -> Result<Vec<Page>> {
    let body = json!({ "urls": urls, "text": true });
    let request = call
        .client
        .post(format!("{}/contents", endpoint.base_url))
        .header("x-api-key", &endpoint.key)
        .json(&body);
    let data: ExaResponse = call.json(request, "Exa").await?;
    let pages = urls
        .iter()
        .map(|url| {
            let failure = data
                .statuses
                .iter()
                .find(|s| &s.id == url && s.status != "success");
            if let Some(status) = failure {
                let tag = status
                    .error
                    .as_ref()
                    .and_then(|e| e["tag"].as_str())
                    .unwrap_or(&status.status);
                return Page::failed(url, tag);
            }
            let found = data
                .results
                .iter()
                .find(|r| r.url.as_ref() == Some(url) || r.id.as_ref() == Some(url));
            match found {
                Some(r) => Page {
                    url: url.clone(),
                    title: r.title.clone(),
                    content: r.text.clone(),
                    error: None,
                },
                None => Page::failed(url, "not returned"),
            }
        })
        .collect();
    Ok(pages)
}
