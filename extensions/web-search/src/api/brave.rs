//! Brave `GET /res/v1/web/search`.
//! Docs: https://api-dashboard.search.brave.com/app/documentation/web-search/get-started

use anyhow::Result;
use serde::Deserialize;

use super::{Endpoint, Found, join_snippets};
use crate::http::Call;
use crate::pages::Hit;

#[derive(Deserialize, Default)]
#[serde(default)]
struct Response {
    web: Web,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Web {
    results: Vec<BraveHit>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct BraveHit {
    title: Option<String>,
    url: Option<String>,
    description: Option<String>,
    age: Option<String>,
    extra_snippets: Vec<String>,
}

pub async fn search(call: &Call, endpoint: &Endpoint, query: &str) -> Result<Found> {
    let count = endpoint.max_results.to_string();
    let request = call
        .client
        .get(format!("{}/res/v1/web/search", endpoint.base_url))
        .query(&[("q", query), ("count", count.as_str())])
        .header("accept", "application/json")
        .header("x-subscription-token", &endpoint.key);
    let data: Response = call.json(request, "Brave").await?;
    let hits = data
        .web
        .results
        .into_iter()
        .filter_map(|r| {
            let mut snippets: Vec<String> = r.description.into_iter().collect();
            snippets.extend(r.extra_snippets);
            let content = join_snippets(&snippets);
            Some(Hit {
                url: r.url?,
                title: r.title,
                content,
                published: r.age,
            })
        })
        .collect();
    Ok(Found { hits, answer: None })
}
