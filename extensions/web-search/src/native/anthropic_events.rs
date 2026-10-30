//! Collects text, citations, and search results from Anthropic Messages stream events.

use anyhow::{Result, anyhow, bail};
use serde_json::Value;

use crate::citations::{Citation, apply_text_citations};
use crate::http::Call;
use crate::results::{
    Detail, SearchCall, StreamResult, derive_sources, merge_metadata, normalize_cited_sources,
    push_unique_detail, push_unique_string, sanitize, title_from_url,
};

#[derive(Default)]
pub struct State {
    kind: &'static str,
    label: &'static str,
    deepseek: bool,
    text: String,
    citations: Vec<Citation>,
    events: Vec<String>,
    calls: Vec<SearchCall>,
    results: Vec<Detail>,
}

impl State {
    pub fn new(kind: &'static str, label: &'static str, deepseek: bool) -> Self {
        Self {
            kind,
            label,
            deepseek,
            ..Self::default()
        }
    }

    fn source(&mut self, source: &Value, tool_use_id: &Value) {
        let Some(url) = source["url"].as_str().filter(|u| !u.is_empty()) else {
            return;
        };
        let title = source["title"]
            .as_str()
            .map_or_else(|| title_from_url(url), str::to_owned);
        self.citations.push(Citation {
            title: title.clone(),
            url: url.to_owned(),
            ..Citation::default()
        });
        let kind = source["type"].as_str().unwrap_or("web_search_result");
        let mut detail = Detail::web(
            &title,
            url,
            &format!("{}.web_search_tool_result", self.kind),
            kind,
        );
        detail.page_age = source
            .get("page_age")
            .or(source.get("pageAge"))
            .and_then(Value::as_str)
            .map(Into::into);
        let mut raw = source.clone();
        raw["toolUseId"] = tool_use_id.clone();
        detail.raw = Some(raw);
        push_unique_detail(&mut self.results, detail);
    }

    fn block_start(&mut self, block: &Value, call: &Call) -> Result<()> {
        match block["type"].as_str().unwrap_or_default() {
            "text" => {
                self.text
                    .push_str(block["text"].as_str().unwrap_or_default());
                call.progress(&self.text);
            }
            "server_tool_use" if block["name"] == "web_search" => {
                push_unique_string(
                    &mut self.events,
                    &format!(
                        "{}.content_block_start.server_tool_use.web_search",
                        self.kind
                    ),
                );
                self.calls.push(SearchCall {
                    id: block["id"].as_str().map(Into::into),
                    provider: self.kind.to_owned(),
                    status: Some("in_progress".to_owned()),
                    action_type: Some("web_search".to_owned()),
                    queries: block["input"]["query"].as_str().map(|q| vec![q.to_owned()]),
                    raw: Some(block.clone()),
                    ..SearchCall::default()
                });
                if self.text.is_empty() {
                    call.progress(&format!("Searching the web with {}...", self.label));
                }
            }
            "web_search_tool_result" => {
                push_unique_string(
                    &mut self.events,
                    &format!("{}.content_block_start.web_search_tool_result", self.kind),
                );
                let id = block["tool_use_id"].as_str().map(str::to_owned);
                match self.calls.iter_mut().find(|c| c.id.is_some() && c.id == id) {
                    Some(existing) => existing.status = Some("completed".to_owned()),
                    None => self.calls.push(SearchCall {
                        id,
                        provider: self.kind.to_owned(),
                        status: Some("completed".to_owned()),
                        action_type: Some("web_search".to_owned()),
                        raw: Some(block.clone()),
                        ..SearchCall::default()
                    }),
                }
                if let Some(list) = block["content"].as_array() {
                    list.iter()
                        .for_each(|result| self.source(result, &block["tool_use_id"]));
                } else if block["content"]["type"] == "web_search_tool_result_error" {
                    let code = block["content"]["error_code"].as_str().map(str::to_owned);
                    if self.deepseek {
                        bail!(
                            "DeepSeek web search failed: {}",
                            code.as_deref().unwrap_or("unknown error")
                        );
                    }
                    let source = format!("{}.web_search_tool_result_error", self.kind);
                    let detail = Detail {
                        status: code,
                        source: Some(source),
                        kind: Some("web_search_tool_result_error".into()),
                        raw: Some(block.clone()),
                        ..Detail::default()
                    };
                    push_unique_detail(&mut self.results, detail);
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn handle(&mut self, event: &Value, call: &Call) -> Result<()> {
        match event["type"].as_str().unwrap_or_default() {
            "content_block_start" => self.block_start(&event["content_block"], call)?,
            "content_block_delta" => {
                let delta = &event["delta"];
                if delta["type"] == "text_delta" {
                    self.text
                        .push_str(delta["text"].as_str().unwrap_or_default());
                    call.progress(&self.text);
                } else if delta["type"] == "citations_delta" {
                    let c = &delta["citation"];
                    let Some(url) = c["url"]
                        .as_str()
                        .filter(|_| c["type"] == "web_search_result_location")
                    else {
                        return Ok(());
                    };
                    let title = c["title"]
                        .as_str()
                        .map_or_else(|| title_from_url(url), str::to_owned);
                    let cited = c["cited_text"].as_str().map(str::to_owned);
                    let mut detail = Detail::web(
                        &title,
                        url,
                        &format!("{}.citations_delta", self.kind),
                        "web_search_result_location",
                    );
                    detail.cited_text.clone_from(&cited);
                    detail.raw = Some(c.clone());
                    self.citations.push(Citation {
                        title,
                        url: url.to_owned(),
                        end_index: None,
                        cited_text: cited,
                    });
                    push_unique_detail(&mut self.results, detail);
                }
            }
            "error" => {
                let message = event["error"]["message"].as_str().map(str::to_owned);
                return Err(anyhow!(message.unwrap_or_else(|| event.to_string())));
            }
            _ => {}
        }
        Ok(())
    }

    pub fn finish(mut self) -> StreamResult {
        let answer = if self.text.is_empty() {
            "No answer available."
        } else {
            &self.text
        };
        let (text, cited) = apply_text_citations(answer, &self.citations);
        let source = format!("{}.citation", self.kind);
        let details: Vec<Detail> = self
            .citations
            .iter()
            .map(|c| Detail {
                cited_text: c.cited_text.clone(),
                ..Detail::web(&c.title, &c.url, &source, "citation")
            })
            .collect();
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
            provider_kind: self.kind.to_owned(),
            native_search_used: !self.events.is_empty()
                || !self.calls.is_empty()
                || !results.is_empty(),
            native_search_events: self.events,
            search_queries: self
                .calls
                .iter()
                .flat_map(|c| c.queries.clone().unwrap_or_default())
                .collect(),
            native_search_calls: self.calls,
            search_results: results,
            citations,
            ..StreamResult::default()
        }
    }
}
