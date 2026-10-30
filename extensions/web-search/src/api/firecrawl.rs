//! Firecrawl `POST /v2/search` and `POST /v2/scrape`; `baseUrl` also serves self-hosted Firecrawl.
//! Docs: https://docs.firecrawl.dev/api-reference/endpoint/search, https://docs.firecrawl.dev/api-reference/endpoint/scrape

use anyhow::{Result, anyhow};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Endpoint, Found};
use crate::http::Call;
use crate::pages::{Hit, Page};

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchResponse {
    success: Option<bool>,
    error: Option<String>,
    data: Value,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WebHit {
    title: Option<String>,
    url: Option<String>,
    description: Option<String>,
    markdown: Option<String>,
}

fn failure(success: Option<bool>, error: Option<String>) -> Result<()> {
    match success {
        Some(false) => Err(anyhow!(
            "Firecrawl error: {}",
            error.unwrap_or_else(|| "request failed".to_owned())
        )),
        _ => Ok(()),
    }
}

pub async fn search(call: &Call, endpoint: &Endpoint, query: &str) -> Result<Found> {
    let body = json!({ "query": query, "limit": endpoint.max_results, "sources": ["web"] });
    let request = call
        .client
        .post(format!("{}/v2/search", endpoint.base_url))
        .bearer_auth(&endpoint.key)
        .json(&body);
    let data: SearchResponse = call.json(request, "Firecrawl").await?;
    failure(data.success, data.error)?;
    let list = data.data.get("web").cloned().unwrap_or(data.data);
    let hits: Vec<WebHit> = serde_json::from_value(list).unwrap_or_default();
    let hits = hits
        .into_iter()
        .filter_map(|r| {
            Some(Hit {
                url: r.url?,
                title: r.title,
                content: r.description.or(r.markdown),
                published: None,
            })
        })
        .collect();
    Ok(Found { hits, answer: None })
}

async fn scrape(call: &Call, endpoint: &Endpoint, url: &str) -> Result<Page> {
    let body = json!({ "url": url, "formats": ["markdown"], "onlyMainContent": true });
    let request = call
        .client
        .post(format!("{}/v2/scrape", endpoint.base_url))
        .bearer_auth(&endpoint.key)
        .json(&body);
    let data: SearchResponse = call.json(request, "Firecrawl").await?;
    failure(data.success, data.error)?;
    let meta = &data.data["metadata"];
    let status = meta["statusCode"].as_u64().unwrap_or(200);
    if status >= 400 {
        return Ok(Page::failed(url, format!("HTTP {status}")));
    }
    Ok(Page {
        url: url.to_owned(),
        title: meta["title"].as_str().map(str::to_owned),
        content: data.data["markdown"].as_str().map(str::to_owned),
        error: None,
    })
}

pub async fn scrape_all(call: &Call, endpoint: &Endpoint, urls: &[String]) -> Vec<Page> {
    let mut pages = Vec::new();
    for url in urls {
        pages.push(
            scrape(call, endpoint, url)
                .await
                .unwrap_or_else(|err| Page::failed(url, err.to_string())),
        );
    }
    pages
}
