//! Results of search APIs (a hit list) and page fetches, in the shape of upstream Ollama.

use serde_json::{Value, json};

use crate::results::{Detail, Source, StreamResult, title_from_url};

const FETCH_CONTENT_LIMIT: usize = 8000;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hit {
    pub title: Option<String>,
    pub url: String,
    pub content: Option<String>,
    pub published: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    pub url: String,
    pub title: Option<String>,
    pub content: Option<String>,
    /// `None` when the fetch succeeded.
    pub error: Option<String>,
}

impl Page {
    pub fn failed(url: &str, error: impl Into<String>) -> Self {
        Self {
            url: url.to_owned(),
            error: Some(error.into()),
            ..Self::default()
        }
    }

    fn label(&self) -> &str {
        self.title
            .as_deref()
            .filter(|t| !t.is_empty())
            .unwrap_or(&self.url)
    }

    fn source(&self) -> Source {
        let title = self.title.clone().filter(|t| !t.is_empty());
        Source {
            title: title.unwrap_or_else(|| title_from_url(&self.url)),
            url: self.url.clone(),
        }
    }
}

pub fn truncate_content(text: &str) -> String {
    match text.char_indices().nth(FETCH_CONTENT_LIMIT) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

pub fn url_metadata(pages: &[Page]) -> Value {
    let items: Vec<Value> = pages
        .iter()
        .map(|page| {
            let status = if page.error.is_none() {
                "URL_RETRIEVAL_STATUS_SUCCESS"
            } else {
                "URL_RETRIEVAL_STATUS_ERROR"
            };
            json!({ "retrievedUrl": page.url, "urlRetrievalStatus": status })
        })
        .collect();
    json!({ "urlMetadata": items })
}

/// A search answer listing each hit; `answer` (Tavily) goes first.
pub fn search_result(
    query: &str,
    label: &str,
    kind: &str,
    hits: &[Hit],
    answer: Option<&str>,
) -> StreamResult {
    let mut lines = vec![format!("Search results for \"{query}\" via {label}:")];
    if let Some(answer) = answer.filter(|a| !a.trim().is_empty()) {
        lines.push(format!("\nAnswer: {}", answer.trim()));
    }
    if hits.is_empty() {
        lines.push("No results found.".to_owned());
    }
    let mut sources = Vec::new();
    let mut results = Vec::new();
    for (i, hit) in hits.iter().enumerate() {
        let title = hit.title.clone().filter(|t| !t.is_empty());
        let mut line = format!(
            "\n{}. {}\n   {}",
            i + 1,
            title.as_deref().unwrap_or(&hit.url),
            hit.url
        );
        if let Some(date) = &hit.published {
            line.push_str(&format!("\n   Published: {date}"));
        }
        if let Some(content) = hit.content.as_ref().filter(|c| !c.is_empty()) {
            line.push_str(&format!("\n   {content}"));
        }
        lines.push(line);
        let title = title.unwrap_or_else(|| title_from_url(&hit.url));
        sources.push(Source {
            title: title.clone(),
            url: hit.url.clone(),
        });
        let mut detail = Detail::web(
            &title,
            &hit.url,
            &format!("{kind}.web_search"),
            "search_result",
        );
        detail.cited_text.clone_from(&hit.content);
        detail.query = Some(query.to_owned());
        detail.page_age.clone_from(&hit.published);
        results.push(detail);
    }
    StreamResult {
        text: lines.join("\n"),
        sources,
        provider_kind: kind.to_owned(),
        native_search_used: true,
        search_queries: vec![query.to_owned()],
        search_results: results,
        ..StreamResult::default()
    }
}

/// Appends fetched pages to a search answer (`web_search` with `urls`).
pub fn add_pages(result: &mut StreamResult, pages: &[Page]) {
    for page in pages.iter().filter(|p| p.error.is_none()) {
        let content = truncate_content(page.content.as_deref().unwrap_or_default());
        result.text.push_str(&format!(
            "\n\n## Fetched: {}\n{}\n\n{content}",
            page.label(),
            page.url
        ));
        if !result.sources.iter().any(|s| s.url == page.url) {
            result.sources.push(page.source());
        }
    }
    result.url_context_metadata = Some(url_metadata(pages));
}

/// The `url_context` answer: each page's text under its title, failures listed.
pub fn url_context_result(query: &str, kind: &str, pages: &[Page]) -> StreamResult {
    let mut lines = vec![format!("URL contents for: {query}")];
    let mut sources = Vec::new();
    let mut results = Vec::new();
    for page in pages {
        if let Some(error) = &page.error {
            lines.push(format!("\n## Failed: {}\n{error}", page.url));
            continue;
        }
        let content = truncate_content(page.content.as_deref().unwrap_or_default());
        lines.push(format!("\n## {}\n{}\n\n{content}", page.label(), page.url));
        sources.push(page.source());
        results.push(Detail {
            title: page.title.clone(),
            url: Some(page.url.clone()),
            cited_text: Some(content),
            source: Some(format!("{kind}.web_fetch")),
            kind: Some("fetched_page".to_owned()),
            ..Detail::default()
        });
    }
    StreamResult {
        text: lines.join("\n"),
        sources,
        provider_kind: kind.to_owned(),
        search_results: results,
        url_context_metadata: Some(url_metadata(pages)),
        ..StreamResult::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_result_lists_hits() {
        let hits = [Hit {
            title: Some("A".into()),
            url: "https://a.com/".into(),
            content: Some("snip".into()),
            published: None,
        }];
        let result = search_result("q", "Exa", "exa", &hits, Some("short"));
        assert_eq!(
            result.text,
            "Search results for \"q\" via Exa:\n\nAnswer: short\n\n1. A\n   https://a.com/\n   snip"
        );
        assert_eq!(result.sources[0].title, "A");
    }

    #[test]
    fn url_context_marks_failures() {
        let pages = [
            Page {
                url: "https://a.com/".into(),
                content: Some("body".into()),
                ..Page::default()
            },
            Page::failed("https://b.com/", "404"),
        ];
        let result = url_context_result("q", "exa", &pages);
        assert!(result.text.contains("## Failed: https://b.com/\n404"));
        let meta = result.url_context_metadata.unwrap();
        assert_eq!(
            meta["urlMetadata"][1]["urlRetrievalStatus"],
            "URL_RETRIEVAL_STATUS_ERROR"
        );
    }
}
