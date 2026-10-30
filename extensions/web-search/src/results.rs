//! Result types shared by every provider, with URL cleanup and dedupe helpers.

use serde::Serialize;
use serde_json::Value;

pub use crate::urls::{is_junk_url, normalize_search_url, title_from_url};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Source {
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_age: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cited_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

impl Detail {
    pub fn web(title: &str, url: &str, source: &str, kind: &str) -> Self {
        Self {
            title: Some(title.to_owned()),
            url: Some(url.to_owned()),
            source: Some(source.to_owned()),
            kind: Some(kind.to_owned()),
            ..Self::default()
        }
    }

    fn key(&self) -> String {
        let field = |value: &Option<String>| value.clone().unwrap_or_default();
        format!(
            "{}\t{}\t{}\t{}\t{}",
            field(&self.url),
            field(&self.title),
            field(&self.query),
            field(&self.cited_text),
            field(&self.kind)
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchCall {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queries: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub urls: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

/// What a provider call returns before formatting.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StreamResult {
    pub text: String,
    pub sources: Vec<Source>,
    pub provider_kind: String,
    pub native_search_used: bool,
    pub native_search_events: Vec<String>,
    pub native_search_calls: Vec<SearchCall>,
    pub search_queries: Vec<String>,
    pub search_results: Vec<Detail>,
    pub citations: Vec<Detail>,
    pub grounding_metadata: Option<Value>,
    pub url_context_metadata: Option<Value>,
}

pub fn push_unique_source(sources: &mut Vec<Source>, title: &str, url: &str) -> usize {
    let title = if title.is_empty() { "Unknown" } else { title };
    if let Some(index) = sources
        .iter()
        .position(|s| s.url == url && s.title == title)
    {
        return index;
    }
    sources.push(Source {
        title: title.to_owned(),
        url: url.to_owned(),
    });
    sources.len() - 1
}

pub fn push_unique_string(values: &mut Vec<String>, value: &str) {
    if !value.is_empty() && !values.iter().any(|v| v == value) {
        values.push(value.to_owned());
    }
}

pub fn push_unique_detail(results: &mut Vec<Detail>, detail: Detail) {
    let key = detail.key();
    if !results.iter().any(|item| item.key() == key) {
        results.push(detail);
    }
}

pub fn sanitize(results: Vec<Detail>) -> Vec<Detail> {
    let mut sanitized = Vec::new();
    for mut result in results {
        result.url = result.url.map(|url| normalize_search_url(&url));
        if result.url.is_some() && is_junk_url(result.url.as_deref()) {
            continue;
        }
        push_unique_detail(&mut sanitized, result);
    }
    sanitized
}

pub fn derive_sources(search_results: &[Detail], citations: &[Detail]) -> Vec<Source> {
    let mut sources = Vec::new();
    for item in citations.iter().chain(search_results) {
        let Some(url) = &item.url else { continue };
        let url = normalize_search_url(url);
        if is_junk_url(Some(&url)) {
            continue;
        }
        let title = item.title.clone().unwrap_or_else(|| title_from_url(&url));
        push_unique_source(&mut sources, &title, &url);
    }
    sources
}

pub fn normalize_cited_sources(sources: Vec<Source>) -> Vec<Source> {
    sources
        .into_iter()
        .map(|source| Source {
            url: normalize_search_url(&source.url),
            ..source
        })
        .filter(|source| !is_junk_url(Some(&source.url)))
        .collect()
}

/// Fills missing fields of `results` from `extras` with the same URL.
pub fn merge_metadata(results: &mut [Detail], extras: &[Detail]) {
    for extra in extras {
        let Some(url) = &extra.url else { continue };
        let Some(existing) = results
            .iter_mut()
            .find(|item| item.url.as_ref() == Some(url))
        else {
            continue;
        };
        for (target, value) in [
            (&mut existing.title, &extra.title),
            (&mut existing.query, &extra.query),
            (&mut existing.cited_text, &extra.cited_text),
            (&mut existing.status, &extra.status),
            (&mut existing.kind, &extra.kind),
            (&mut existing.source, &extra.source),
        ] {
            if target.is_none() {
                target.clone_from(value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_unique_sources_citations_first() {
        let a = Detail::web("A", "https://a.com/?utm_x=1", "s", "web");
        let b = Detail::web("B", "https://b.com/", "s", "web");
        let sources = derive_sources(&[b.clone(), a.clone()], &[a]);
        let urls: Vec<&str> = sources.iter().map(|s| s.url.as_str()).collect();
        assert_eq!(urls, ["https://a.com/", "https://b.com/"]);
    }
}
