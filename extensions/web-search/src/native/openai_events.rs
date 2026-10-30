//! Collects text, citations, and `web_search_call` items from Responses stream events.

use anyhow::{Result, anyhow};
use serde_json::Value;

use crate::citations::{Citation, apply_index_citations, preserve_inline_citations};
use crate::http::Call;
use crate::results::{
    Detail, SearchCall, StreamResult, derive_sources, merge_metadata, normalize_cited_sources,
    push_unique_detail, push_unique_string, sanitize, title_from_url,
};

pub struct Collector {
    provider: &'static str,
    label: &'static str,
    codex: bool,
    text: String,
    citations: Vec<Citation>,
    events: Vec<String>,
    calls: Vec<SearchCall>,
    queries: Vec<String>,
    results: Vec<Detail>,
}

fn text(value: &Value) -> Option<String> {
    value.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

fn citation(annotation: &Value) -> Option<Citation> {
    let nested = annotation
        .get("url_citation")
        .or_else(|| annotation.get("urlCitation"));
    let pick = |key: &str| {
        annotation
            .get(key)
            .filter(|v| !v.is_null())
            .or_else(|| nested?.get(key))
    };
    let url = pick("url").and_then(text)?;
    let title = pick("title")
        .and_then(text)
        .unwrap_or_else(|| title_from_url(&url));
    let end = pick("end_index")
        .or_else(|| pick("endIndex"))
        .and_then(Value::as_u64);
    Some(Citation {
        title,
        url,
        end_index: end.map(|e| e as usize),
        cited_text: None,
    })
}

impl Collector {
    pub fn new(xai: bool, codex: bool) -> Self {
        Self {
            provider: if xai { "xai" } else { "openai" },
            label: if xai { "xAI" } else { "OpenAI" },
            codex,
            text: String::new(),
            citations: Vec::new(),
            events: Vec::new(),
            calls: Vec::new(),
            queries: Vec::new(),
            results: Vec::new(),
        }
    }

    fn annotation(&mut self, annotation: &Value) {
        if annotation["type"] == "url_citation" {
            self.citations.extend(citation(annotation));
        }
    }

    fn search_call(&mut self, item: &Value) {
        if item["type"] != "web_search_call" {
            return;
        }
        let action = &item["action"];
        let mut call = SearchCall {
            id: text(&item["id"]),
            provider: self.provider.to_owned(),
            status: text(&item["status"]),
            action_type: text(&action["type"]),
            raw: Some(item.clone()),
            ..SearchCall::default()
        };
        let queries: Option<Vec<String>> = match (&action["queries"], &action["query"]) {
            (Value::Array(list), _) => Some(list.iter().filter_map(text).collect()),
            (_, Value::String(query)) => Some(vec![query.clone()]),
            _ => None,
        };
        for query in queries.iter().flatten() {
            push_unique_string(&mut self.queries, query);
        }
        call.queries = queries;
        let source_name = format!("{}.web_search_call.action", self.provider);
        if let Some(sources) = action["sources"].as_array() {
            call.urls = Some(sources.iter().filter_map(|s| text(&s["url"])).collect());
            for source in sources {
                let Some(url) = text(&source["url"]) else {
                    continue;
                };
                let title = ["title", "display_name", "name"]
                    .iter()
                    .find_map(|k| text(&source[*k]));
                let kind = text(&source["type"]).unwrap_or_else(|| "url".to_owned());
                let title = title.unwrap_or_else(|| title_from_url(&url));
                let mut detail =
                    Detail::web(&title, &url, &format!("{source_name}.sources"), &kind);
                detail.raw = Some(source.clone());
                push_unique_detail(&mut self.results, detail);
            }
        }
        if let Some(url) = text(&action["url"]) {
            call.urls.get_or_insert_with(Vec::new).push(url.clone());
            let kind = text(&action["type"]).unwrap_or_default();
            let source = format!("{source_name}.{kind}");
            let mut detail = Detail::web(&title_from_url(&url), &url, &source, &kind);
            detail.raw = Some(action.clone());
            push_unique_detail(&mut self.results, detail);
        }
        match self
            .calls
            .iter_mut()
            .find(|c| c.id.is_some() && c.id == call.id)
        {
            Some(existing) => {
                existing.status = call.status.or(existing.status.take());
                existing.action_type = call.action_type.or(existing.action_type.take());
                existing.queries = call.queries.or(existing.queries.take());
                existing.urls = call.urls.or(existing.urls.take());
                existing.raw = call.raw;
            }
            None => self.calls.push(call),
        }
    }

    fn response(&mut self, response: &Value) {
        for item in response["output"].as_array().into_iter().flatten() {
            self.search_call(item);
            if item["type"] != "message" {
                continue;
            }
            for content in item["content"].as_array().into_iter().flatten() {
                if content["type"] == "output_text" {
                    for annotation in content["annotations"].as_array().into_iter().flatten() {
                        self.annotation(annotation);
                    }
                }
            }
        }
    }

    /// Returns `true` to stop reading (Codex keeps the stream open after completion).
    pub fn handle(&mut self, event: &Value, call: &Call) -> Result<bool> {
        let kind = event["type"].as_str().unwrap_or_default();
        match kind {
            "error" | "response.failed" => {
                let message = text(&event["message"])
                    .or_else(|| text(&event["error"]["message"]))
                    .or_else(|| text(&event["response"]["error"]["message"]));
                return Err(anyhow!(message.unwrap_or_else(|| event.to_string())));
            }
            "response.output_text.delta" => {
                self.text
                    .push_str(event["delta"].as_str().unwrap_or_default());
                call.progress(&self.text);
            }
            "response.output_text.annotation.added" => self.annotation(&event["annotation"]),
            "response.output_item.added" | "response.output_item.done" => {
                self.search_call(&event["item"])
            }
            "response.completed" | "response.done" | "response.incomplete" => {
                self.response(&event["response"]);
                return Ok(self.codex);
            }
            _ if event["response"]["status"] == "incomplete" => {
                self.response(&event["response"]);
                return Ok(self.codex);
            }
            _ if kind.starts_with("response.web_search_call.") => {
                push_unique_string(&mut self.events, kind);
                let status = kind
                    .trim_start_matches("response.web_search_call.")
                    .to_owned();
                let id = text(&event["item_id"]);
                match self.calls.iter_mut().find(|c| c.id.is_some() && c.id == id) {
                    Some(existing) => existing.status = Some(status.clone()),
                    None => self.calls.push(SearchCall {
                        id,
                        provider: self.provider.to_owned(),
                        status: Some(status.clone()),
                        raw: Some(event.clone()),
                        ..SearchCall::default()
                    }),
                }
                if status == "searching" && self.text.is_empty() {
                    call.progress(&format!("Searching the web with {}...", self.label));
                }
            }
            _ => {}
        }
        Ok(false)
    }

    pub fn finish(mut self, grok: bool) -> StreamResult {
        let answer = if self.text.is_empty() {
            "No answer available."
        } else {
            &self.text
        };
        let (text, cited) = if grok {
            preserve_inline_citations(answer, &self.citations)
        } else {
            apply_index_citations(answer, &self.citations)
        };
        let source = format!("{}.url_citation", self.provider);
        let details: Vec<Detail> = self
            .citations
            .iter()
            .map(|c| Detail {
                raw: None,
                ..Detail::web(&c.title, &c.url, &source, "citation")
            })
            .collect();
        for detail in &details {
            push_unique_detail(&mut self.results, detail.clone());
        }
        merge_metadata(&mut self.results, &details);
        let mut results = sanitize(self.results);
        let citations = sanitize(details);
        merge_metadata(&mut results, &citations);
        let sources = if cited.is_empty() {
            derive_sources(&results, &citations)
        } else {
            normalize_cited_sources(cited)
        };
        StreamResult {
            text,
            sources,
            provider_kind: self.provider.to_owned(),
            native_search_used: !self.events.is_empty()
                || !self.calls.is_empty()
                || !results.is_empty(),
            native_search_events: self.events,
            native_search_calls: self.calls,
            search_queries: self.queries,
            search_results: results,
            citations,
            ..StreamResult::default()
        }
    }
}
