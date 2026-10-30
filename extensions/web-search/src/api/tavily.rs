//! Tavily `POST /search` and `POST /extract`.
//! Docs: https://docs.tavily.com/documentation/api-reference/endpoint/search and `/extract`

use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

use super::{Endpoint, Found};
use crate::http::Call;
use crate::pages::{Hit, Page};

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchResponse {
    answer: Option<String>,
    results: Vec<SearchHit>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchHit {
    title: Option<String>,
    url: Option<String>,
    content: Option<String>,
    published_date: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ExtractResponse {
    results: Vec<Extracted>,
    failed_results: Vec<Failed>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Extracted {
    url: String,
    title: Option<String>,
    raw_content: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Failed {
    url: String,
    error: Option<String>,
}

pub async fn search(call: &Call, endpoint: &Endpoint, query: &str) -> Result<Found> {
    let body = json!({
        "query": query, "max_results": endpoint.max_results,
        "search_depth": "basic", "include_answer": true
    });
    let request = call
        .client
        .post(format!("{}/search", endpoint.base_url))
        .bearer_auth(&endpoint.key)
        .json(&body);
    let data: SearchResponse = call.json(request, "Tavily").await?;
    let hits = data
        .results
        .into_iter()
        .filter_map(|r| {
            Some(Hit {
                url: r.url?,
                title: r.title,
                content: r.content,
                published: r.published_date,
            })
        })
        .collect();
    Ok(Found {
        hits,
        answer: data.answer,
    })
}

pub async fn extract(call: &Call, endpoint: &Endpoint, urls: &[String]) -> Result<Vec<Page>> {
    let body = json!({ "urls": urls, "format": "markdown" });
    let request = call
        .client
        .post(format!("{}/extract", endpoint.base_url))
        .bearer_auth(&endpoint.key)
        .json(&body);
    let data: ExtractResponse = call.json(request, "Tavily").await?;
    let pages = urls
        .iter()
        .map(|url| {
            if let Some(failed) = data.failed_results.iter().find(|f| &f.url == url) {
                return Page::failed(
                    url,
                    failed.error.clone().unwrap_or_else(|| "failed".to_owned()),
                );
            }
            match data.results.iter().find(|r| &r.url == url) {
                Some(r) => Page {
                    url: url.clone(),
                    title: r.title.clone(),
                    content: r.raw_content.clone(),
                    error: None,
                },
                None => Page::failed(url, "not returned"),
            }
        })
        .collect();
    Ok(pages)
}
