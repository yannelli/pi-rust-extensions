//! Gemini `google_search` grounding and `url_context` over `streamGenerateContent`.
//! Docs: https://ai.google.dev/gemini-api/docs/google-search, https://ai.google.dev/gemini-api/docs/url-context

use std::collections::HashMap;

use anyhow::{Result, anyhow};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};

use super::cloudcode;
use crate::http::Call;
use crate::model::Target;
use crate::results::{
    Detail, StreamResult, derive_sources, push_unique_detail, sanitize, title_from_url,
};

const REDIRECT_PREFIX: &str = "https://vertexaisearch.cloud.google.com/grounding-api-redirect/";

pub fn stream_url(target: &Target) -> String {
    let base = target.model.base_url.trim_end_matches('/');
    format!(
        "{base}/models/{}:streamGenerateContent?alt=sse",
        target.model.id
    )
}

pub fn build_headers(target: &Target) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    let defaults = [
        ("content-type", "application/json"),
        ("accept", "text/event-stream"),
    ];
    let defaults = defaults
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()));
    for (name, value) in defaults
        .chain(target.session_headers())
        .chain(target.auth.headers.clone())
    {
        headers.insert(
            HeaderName::from_bytes(name.as_bytes())?,
            HeaderValue::from_str(&value)?,
        );
    }
    if let Some(key) = &target.auth.api_key {
        headers.insert("x-goog-api-key", HeaderValue::from_str(key)?);
    }
    Ok(headers)
}

fn search_details(grounding: Option<&Value>) -> (Vec<String>, Vec<Detail>, Vec<Detail>) {
    let Some(g) = grounding else {
        return Default::default();
    };
    let queries = g["webSearchQueries"].as_array().into_iter().flatten();
    let queries = queries
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let empty = Vec::new();
    let chunks = g["groundingChunks"].as_array().unwrap_or(&empty);
    let detail = |web: &Value, source: &str, kind: &str| {
        let title = web["title"].as_str().unwrap_or("Unknown");
        Detail::web(title, web["uri"].as_str().unwrap_or_default(), source, kind)
    };
    let mut results = Vec::new();
    for (index, chunk) in chunks.iter().enumerate() {
        if chunk["web"].is_object() {
            let mut item = detail(&chunk["web"], "google.groundingChunks", "web");
            let mut raw = chunk["web"].clone();
            raw["index"] = json!(index);
            item.raw = Some(raw);
            push_unique_detail(&mut results, item);
        }
    }
    let mut citations = Vec::new();
    for support in g["groundingSupports"].as_array().into_iter().flatten() {
        for index in support["groundingChunkIndices"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let web = &chunks
                .get(index.as_u64().unwrap_or(u64::MAX) as usize)
                .unwrap_or(&Value::Null)["web"];
            if !web.is_object() {
                continue;
            }
            let mut item = detail(web, "google.groundingSupports", "citation");
            item.cited_text = support["segment"]["text"].as_str().map(str::to_owned);
            item.raw = Some(support.clone());
            push_unique_detail(&mut citations, item);
        }
    }
    (queries, results, citations)
}

/// Grounding links point at a Google redirect; a HEAD without redirects reveals the target.
async fn resolve_redirects(call: &Call, items: &mut [&mut Vec<Detail>]) {
    let mut urls: Vec<String> = items
        .iter()
        .flat_map(|list| list.iter())
        .filter_map(|d| d.url.clone())
        .collect();
    urls.retain(|url| url.starts_with(REDIRECT_PREFIX));
    urls.sort();
    urls.dedup();
    urls.truncate(20);
    if urls.is_empty() {
        return;
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build();
    let Ok(client) = client else { return };
    let mut resolved = HashMap::new();
    for url in urls {
        let head = client.head(&url).send();
        let answer = tokio::select! {
            _ = call.cancel.cancelled() => return,
            r = tokio::time::timeout(std::time::Duration::from_secs(10), head) => r,
        };
        let Ok(Ok(response)) = answer else { continue };
        if let Some(location) = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
        {
            resolved.insert(url, location.to_owned());
        }
    }
    for item in items.iter_mut().flat_map(|list| list.iter_mut()) {
        let Some(target) = item.url.as_ref().and_then(|url| resolved.get(url)) else {
            continue;
        };
        item.url = Some(target.clone());
        if item.title.as_deref().is_none_or(|t| t == "Unknown") {
            item.title = Some(title_from_url(target));
        }
    }
}

/// `body` is a Gemini `generateContent` request (`contents`, optional `tools`).
pub async fn generate(call: &Call, target: &Target, body: &Value) -> Result<StreamResult> {
    let request = match cloudcode::flavor(&target.model) {
        Some(flavor) => call
            .client
            .post(cloudcode::url(flavor, &target.model))
            .headers(cloudcode::headers(flavor, target)?)
            .json(&cloudcode::body(flavor, target, body)),
        None => call
            .client
            .post(stream_url(target))
            .headers(build_headers(target)?)
            .json(body),
    };
    let response = call.send(request, "Google").await?;
    let mut text = String::new();
    let (mut grounding, mut url_context) = (None, None);
    call.sse(response, |event| {
        let chunk = event.data;
        if let Some(error) = chunk.get("error").filter(|e| !e.is_null()) {
            let message = error["message"]
                .as_str()
                .map_or_else(|| error.to_string(), str::to_owned);
            let code = [&error["code"], &error["status"]]
                .into_iter()
                .find(|v| !v.is_null());
            let code = code.map_or_else(
                || "unknown".to_owned(),
                |v| v.as_str().map_or_else(|| v.to_string(), str::to_owned),
            );
            return Err(anyhow!("API error ({code}): {message}"));
        }
        let data = chunk.get("response").unwrap_or(&chunk);
        let candidate = &data["candidates"][0];
        for part in candidate["content"]["parts"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(piece) = part["text"].as_str().filter(|t| !t.is_empty()) {
                text.push_str(piece);
                call.progress(&text);
            }
        }
        if candidate["groundingMetadata"].is_object() {
            grounding = Some(candidate["groundingMetadata"].clone());
        }
        let meta = candidate
            .get("urlContextMetadata")
            .or(candidate.get("url_context_metadata"));
        if let Some(meta) = meta.filter(|m| !m.is_null()) {
            url_context = Some(meta.clone());
        }
        Ok(false)
    })
    .await?;
    let (queries, mut results, mut citations) = search_details(grounding.as_ref());
    resolve_redirects(call, &mut [&mut results, &mut citations]).await;
    let results = sanitize(results);
    let citations = sanitize(citations);
    Ok(StreamResult {
        text: if text.is_empty() {
            "No answer available.".to_owned()
        } else {
            text
        },
        sources: derive_sources(&results, &citations),
        provider_kind: "google".to_owned(),
        native_search_used: !queries.is_empty() || !results.is_empty(),
        native_search_events: if queries.is_empty() {
            vec![]
        } else {
            vec!["google.groundingMetadata.webSearchQueries".to_owned()]
        },
        search_queries: queries,
        search_results: results,
        citations,
        grounding_metadata: grounding,
        url_context_metadata: url_context,
        ..StreamResult::default()
    })
}

fn is_youtube(url: &str) -> bool {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let id = ["youtube.com/watch?v=", "youtube.com/embed/", "youtu.be/"]
        .iter()
        .find_map(|p| rest.strip_prefix(p));
    id.is_some_and(|id| {
        id.len() >= 11
            && id[..11]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

/// `url_context` request; on `google-generative-ai`, YouTube links become `file_data` video parts.
pub fn url_context_body(query: &str, urls: &[String], videos_as_files: bool) -> Value {
    let (videos, pages): (Vec<&String>, Vec<&String>) = urls
        .iter()
        .partition(|url| videos_as_files && is_youtube(url));
    let tools = json!([{ "url_context": {} }]);
    if videos.is_empty() {
        let prompt = format!("{query}\n\nURLs:\n{}", urls.join("\n"));
        return json!({ "contents": [{ "role": "user", "parts": [{ "text": prompt }] }], "tools": tools });
    }
    let mut parts: Vec<Value> = videos
        .iter()
        .map(|url| json!({ "file_data": { "file_uri": url, "mime_type": "video/mp4" } }))
        .collect();
    let mut prompt = query.to_owned();
    if !pages.is_empty() {
        let pages: Vec<&str> = pages.iter().map(|s| s.as_str()).collect();
        prompt.push_str(&format!("\n\nURLs:\n{}", pages.join("\n")));
    }
    parts.push(json!({ "text": prompt }));
    json!({ "contents": [{ "role": "user", "parts": parts }], "tools": tools })
}

pub fn web_search_body(prompt: &str, with_urls: bool) -> Value {
    let mut tools = vec![json!({ "google_search": {} })];
    if with_urls {
        tools.push(json!({ "url_context": {} }));
    }
    json!({ "contents": [{ "role": "user", "parts": [{ "text": prompt }] }], "tools": tools })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_links_become_video_parts() {
        let urls = vec![
            "https://youtu.be/dQw4w9WgXcQ".to_owned(),
            "https://a.com".to_owned(),
        ];
        let body = url_context_body("sum", &urls, true);
        let parts = &body["contents"][0]["parts"];
        assert_eq!(
            parts[0]["file_data"]["file_uri"],
            "https://youtu.be/dQw4w9WgXcQ"
        );
        assert_eq!(parts[1]["text"], "sum\n\nURLs:\nhttps://a.com");
        assert!(!is_youtube("https://youtube.com/watch?v=short"));
    }
}
