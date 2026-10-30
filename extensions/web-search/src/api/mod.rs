//! Search APIs with their own keys: Exa, Firecrawl, Brave, and Tavily.

mod brave;
mod exa;
mod firecrawl;
mod tavily;

use anyhow::{Result, bail};

use crate::config::Api;
use crate::http::Call;
use crate::pages::{Page, add_pages, search_result, url_context_result};
use crate::results::StreamResult;

/// A configured API provider: base URL without a trailing slash, key, and result count.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub api: Api,
    pub base_url: String,
    pub key: String,
    pub max_results: u32,
}

#[derive(Debug, Default)]
pub struct Found {
    pub hits: Vec<crate::pages::Hit>,
    pub answer: Option<String>,
}

pub async fn fetch(call: &Call, endpoint: &Endpoint, urls: &[String]) -> Result<Vec<Page>> {
    match endpoint.api {
        Api::Exa => exa::contents(call, endpoint, urls).await,
        Api::Firecrawl => Ok(firecrawl::scrape_all(call, endpoint, urls).await),
        Api::Tavily => tavily::extract(call, endpoint, urls).await,
        Api::Brave => bail!("Brave Search has no page fetch API"),
    }
}

pub async fn web_search(
    call: &Call,
    endpoint: &Endpoint,
    query: &str,
    urls: &[String],
) -> Result<StreamResult> {
    let label = endpoint.api.label();
    call.progress(&format!("Searching the web with {label}..."));
    let found = match endpoint.api {
        Api::Exa => exa::search(call, endpoint, query).await?,
        Api::Firecrawl => firecrawl::search(call, endpoint, query).await?,
        Api::Brave => brave::search(call, endpoint, query).await?,
        Api::Tavily => tavily::search(call, endpoint, query).await?,
    };
    let mut result = search_result(
        query,
        label,
        endpoint.api.id(),
        &found.hits,
        found.answer.as_deref(),
    );
    if !urls.is_empty() && endpoint.api.fetches_urls() {
        call.progress(&format!("Fetching {} URL(s) with {label}...", urls.len()));
        add_pages(&mut result, &fetch(call, endpoint, urls).await?);
    } else if !urls.is_empty() {
        result.text.push_str(&format!(
            "\n\n{label} cannot fetch pages; these URLs were not read:\n{}",
            urls.join("\n")
        ));
    }
    Ok(result)
}

pub async fn url_context(
    call: &Call,
    endpoint: &Endpoint,
    query: &str,
    urls: &[String],
) -> Result<StreamResult> {
    let pages = fetch(call, endpoint, urls).await?;
    Ok(url_context_result(query, endpoint.api.id(), &pages))
}

/// Joins snippet lists into one line.
fn join_snippets(snippets: &[String]) -> Option<String> {
    let parts: Vec<&str> = snippets
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(" ... "))
}
