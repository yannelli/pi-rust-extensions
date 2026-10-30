//! Tool result text and `details`, in the shape of the upstream package.

use pi_ext_sdk::ToolResult;
use serde_json::{Value, json};

use crate::citations::apply_grounding;
use crate::results::{Detail, Source, StreamResult};

const MAX_LINES: usize = 2000;
const MAX_BYTES: usize = 50 * 1024;
const ADDITIONAL_RESULTS_LIMIT: usize = 8;
const SUCCESS: &str = "URL_RETRIEVAL_STATUS_SUCCESS";

/// The first 2000 lines or 50 KB, whichever is less.
pub fn truncate_head(text: &str) -> (String, bool) {
    let mut out = String::new();
    for (count, line) in text.split('\n').enumerate() {
        let extra = usize::from(count > 0) + line.len();
        if count >= MAX_LINES || out.len() + extra > MAX_BYTES {
            return (out, true);
        }
        if count > 0 {
            out.push('\n');
        }
        out.push_str(line);
    }
    (out, false)
}

pub fn format_result(text: &str, details: Value) -> ToolResult {
    let (content, truncated) = truncate_head(text);
    let suffix = if truncated { "\n\n[Truncated]" } else { "" };
    ToolResult::text(format!("{content}{suffix}")).details(details)
}

/// A failure the model can read; `details.error` names it as upstream does.
pub fn error_result(text: &str, details: Value) -> ToolResult {
    format_result(text, details).failed()
}

pub fn exception_result(message: &str) -> ToolResult {
    ToolResult::text(format!("Error: {message}"))
        .details(json!({ "error": true }))
        .failed()
}

fn cited_text(result: &StreamResult) -> (String, Vec<Source>) {
    let (text, sources) = apply_grounding(&result.text, result.grounding_metadata.as_ref());
    let sources = if result.sources.is_empty() {
        sources
    } else {
        result.sources.clone()
    };
    (text, sources)
}

struct UrlStatus {
    retrieved: Vec<String>,
    failed: Vec<Value>,
}

fn url_status(result: &StreamResult) -> UrlStatus {
    let meta = result.url_context_metadata.as_ref();
    let list = meta
        .and_then(|m| m.get("urlMetadata").or_else(|| m.get("url_metadata")))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let field = |m: &Value, a: &str, b: &str| {
        m.get(a)
            .or_else(|| m.get(b))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let url =
        |m: &Value| field(m, "retrievedUrl", "retrieved_url").or_else(|| field(m, "url", "url"));
    let (mut retrieved, mut failed) = (Vec::new(), Vec::new());
    for item in &list {
        let status = field(item, "urlRetrievalStatus", "url_retrieval_status");
        if status.as_deref() == Some(SUCCESS) {
            retrieved.extend(url(item));
        } else {
            failed.push(json!({ "url": url(item), "status": status }));
        }
    }
    UrlStatus { retrieved, failed }
}

fn additional_results(result: &StreamResult, sources: &[Source], dedupe: bool) -> Vec<Detail> {
    let mut seen = Vec::new();
    let mut collected = Vec::new();
    for item in &result.search_results {
        let Some(url) = &item.url else { continue };
        if sources.iter().any(|source| &source.url == url) {
            continue;
        }
        let key = format!("{}\t{url}", item.title.clone().unwrap_or_default());
        if dedupe && seen.contains(&key) {
            continue;
        }
        seen.push(key);
        collected.push(item.clone());
    }
    collected
}

fn append_url_status(summary: &mut String, status: &UrlStatus) {
    if status.failed.is_empty() {
        return;
    }
    summary.push_str(&format!(
        "\n\n## URL Status\n✅ Retrieved: {}\n❌ Failed: {}",
        status.retrieved.len(),
        status.failed.len()
    ));
    for failed in &status.failed {
        let text = |key: &str| failed[key].as_str().unwrap_or("undefined").to_owned();
        summary.push_str(&format!("\n- {}: {}", text("url"), text("status")));
    }
}

fn append_sources(summary: &mut String, sources: &[Source]) {
    if sources.is_empty() || summary.contains("## Sources") {
        return;
    }
    let lines: Vec<String> = sources
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. [{}]({})", i + 1, s.title, s.url))
        .collect();
    summary.push_str(&format!("\n\n## Sources\n{}", lines.join("\n")));
}

fn append_additional(summary: &mut String, results: &[Detail], limit: Option<usize>, meta: bool) {
    if results.is_empty() {
        return;
    }
    let visible = &results[..limit.unwrap_or(results.len()).min(results.len())];
    let lines: Vec<String> = visible
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let label = r
                .title
                .clone()
                .or_else(|| r.url.clone())
                .unwrap_or_else(|| format!("Result {}", i + 1));
            let url = r
                .url
                .as_ref()
                .map(|u| format!(" - {u}"))
                .unwrap_or_default();
            let query = r.query.as_ref().map(|q| format!("query={q}"));
            let parts = [r.source.clone(), r.kind.clone(), r.status.clone(), query];
            let joined: Vec<String> = parts
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect();
            let meta = if meta && !joined.is_empty() {
                format!(" ({})", joined.join(", "))
            } else {
                String::new()
            };
            format!("{}. {label}{url}{meta}", i + 1)
        })
        .collect();
    summary.push_str(&format!(
        "\n\n## Additional Search Results\n{}",
        lines.join("\n")
    ));
    if results.len() > visible.len() {
        summary.push_str(&format!(
            "\n... and {} more results in tool details.",
            results.len() - visible.len()
        ));
    }
}

fn details(
    result: &StreamResult,
    sources: &[Source],
    status: &UrlStatus,
    retrieved: bool,
    model: &str,
) -> Value {
    let queries = if result.search_queries.is_empty() {
        result
            .grounding_metadata
            .as_ref()
            .map(|g| g["webSearchQueries"].clone())
            .filter(|q| !q.is_null())
    } else {
        Some(json!(result.search_queries))
    };
    let count = if result.search_results.is_empty() {
        sources.len()
    } else {
        result.search_results.len()
    };
    json!({
        "sources": sources,
        "providerKind": result.provider_kind,
        "nativeSearchUsed": result.native_search_used,
        "nativeSearchEvents": result.native_search_events,
        "nativeSearchCalls": result.native_search_calls,
        "searchQueries": queries,
        "searchResults": result.search_results,
        "citations": result.citations,
        "retrieved": retrieved.then(|| status.retrieved.clone()),
        "failed": (!status.failed.is_empty()).then(|| status.failed.clone()),
        "model": model,
        "grounded": !sources.is_empty() || !result.search_results.is_empty(),
        "resultCount": count,
    })
}

pub fn format_web_search(result: &StreamResult, model: &str) -> ToolResult {
    let (mut summary, sources) = cited_text(result);
    let additional = additional_results(result, &sources, true);
    let status = url_status(result);
    append_url_status(&mut summary, &status);
    append_sources(&mut summary, &sources);
    append_additional(&mut summary, &additional, None, false);
    let retrieved = !status.retrieved.is_empty();
    format_result(
        &summary,
        details(result, &sources, &status, retrieved, model),
    )
}

pub fn format_url_context(result: &StreamResult, model: &str) -> ToolResult {
    let (mut summary, sources) = cited_text(result);
    let additional = additional_results(result, &sources, false);
    let status = url_status(result);
    let verified = !status.retrieved.is_empty()
        || !status.failed.is_empty()
        || !sources.is_empty()
        || !additional.is_empty();
    append_url_status(&mut summary, &status);
    if !verified {
        let kind = if result.provider_kind.is_empty() {
            "unknown"
        } else {
            &result.provider_kind
        };
        summary.push_str(&format!("\n\n## URL Context Verification\n⚠️ No verified URL context metadata was returned by provider {kind}. Treat the answer as ungrounded unless sources, retrieved URLs, or searchResults are present in tool details."));
    }
    append_sources(&mut summary, &sources);
    append_additional(
        &mut summary,
        &additional,
        Some(ADDITIONAL_RESULTS_LIMIT),
        true,
    );
    format_result(&summary, details(result, &sources, &status, true, model))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_by_lines_and_bytes() {
        let many = "x\n".repeat(3000);
        assert_eq!(truncate_head(&many).0.lines().count(), MAX_LINES);
        let wide = "y".repeat(MAX_BYTES + 10);
        assert!(truncate_head(&wide).1);
        assert_eq!(truncate_head("a\nb"), ("a\nb".to_owned(), false));
    }

    #[test]
    fn web_search_lists_sources_and_failed_urls() {
        let result = StreamResult {
            text: "Answer".into(),
            sources: vec![Source {
                title: "A".into(),
                url: "https://a.com/".into(),
            }],
            provider_kind: "exa".into(),
            url_context_metadata: Some(json!({ "urlMetadata": [
                { "retrievedUrl": "https://b.com/", "urlRetrievalStatus": "URL_RETRIEVAL_STATUS_ERROR" }
            ] })),
            ..StreamResult::default()
        };
        let out = format_web_search(&result, "m");
        let text = out.content[0]["text"].as_str().unwrap();
        assert!(text.starts_with("Answer\n\n## URL Status\n✅ Retrieved: 0\n❌ Failed: 1"));
        assert!(text.ends_with("## Sources\n1. [A](https://a.com/)"));
        let details = out.details.unwrap();
        assert_eq!(details["grounded"], true);
        assert_eq!(details["failed"][0]["url"], "https://b.com/");
    }
}
