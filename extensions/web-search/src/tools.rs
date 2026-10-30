//! `web_search` and `url_context`: names, schemas, and descriptions follow pi-web-search.

use std::sync::Arc;

use pi_ext_sdk::prelude::*;
use serde::Deserialize;

use crate::context::load_config;
use crate::format::{exception_result, format_url_context, format_web_search};
use crate::http::Call;
use crate::plan::{self, Plan};
use crate::{api, native};

pub const WEB_SEARCH: &str = "web_search";
pub const URL_CONTEXT: &str = "url_context";

#[derive(Deserialize)]
pub struct WebSearchArgs {
    pub query: String,
    #[serde(default)]
    pub urls: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub struct UrlContextArgs {
    pub query: String,
    pub urls: Vec<String>,
}

pub fn web_search_spec() -> ToolSpec {
    ToolSpec::new(
        WEB_SEARCH,
        "Search the web using the current supported provider (Google Gemini, xAI Grok, OpenAI, Anthropic, DeepSeek, Ollama, or OpenCode Zen/Go) or a configured search API (Exa, Firecrawl, Brave, Tavily). Optionally include URLs to analyze alongside search results.",
    )
    .label("Web Search")
    .parameters(json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "The search query or question to answer" },
            "urls": {
                "type": "array", "items": { "type": "string" }, "maxItems": 20,
                "description": "Additional URLs to analyze along with search (up to 20)"
            }
        },
        "required": ["query"]
    }))
}

pub fn url_context_spec() -> ToolSpec {
    ToolSpec::new(
        URL_CONTEXT,
        "Analyze the content of up to 20 public URLs using provider-native URL retrieval (Gemini URL Context or Ollama web fetch) or a configured fetch API (Exa, Firecrawl, Tavily). Supports web pages and documents; images and YouTube videos need Gemini.",
    )
    .label("URL Context")
    .parameters(json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "Question or task to perform on the URLs" },
            "urls": {
                "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 20,
                "description": "Public URLs to analyze (web pages, documents, images, YouTube videos, etc)."
            }
        },
        "required": ["query", "urls"]
    }))
}

fn call_for(ctx: &ToolCtx) -> Call {
    let updates = ctx.clone();
    Call::new(
        ctx.cancel_token().clone(),
        Arc::new(move |text: &str| {
            let partial = ToolResult::text(text).details(json!({ "streaming": true }));
            let _ = updates.update(partial);
        }),
    )
}

fn finish(ctx: &ToolCtx, outcome: anyhow::Result<ToolResult>) -> Result<ToolResult> {
    match outcome {
        Ok(result) => Ok(result),
        Err(_) if ctx.is_cancelled() => Err(Error::cancelled()),
        Err(err) => Ok(exception_result(&format!("{err:#}"))),
    }
}

fn check_urls(urls: &[String], min: usize) -> Result<()> {
    if urls.len() < min || urls.len() > 20 {
        return Err(Error::invalid_params(format!(
            "urls must have {min} to 20 items"
        )));
    }
    Ok(())
}

pub async fn web_search(args: WebSearchArgs, ctx: ToolCtx) -> Result<ToolResult> {
    let urls = args.urls.unwrap_or_default();
    check_urls(&urls, 0)?;
    let host = ctx.host().clone();
    let loaded = load_config(&host, Some(ctx.snapshot())).await;
    let plan = plan::web_search(&host, ctx.snapshot(), &loaded).await;
    let call = call_for(&ctx);
    call.progress(&if urls.is_empty() {
        format!("Searching for \"{}\"...", args.query)
    } else {
        format!("Searching and analyzing {} URL(s)...", urls.len())
    });
    let outcome = match plan {
        Plan::Fail(result) => return Ok(result),
        Plan::Native(target) => native::web_search(&call, &target, &args.query, &urls)
            .await
            .map(|result| format_web_search(&result, &target.model.id)),
        Plan::Api(endpoint) => api::web_search(&call, &endpoint, &args.query, &urls)
            .await
            .map(|result| format_web_search(&result, endpoint.api.id())),
    };
    finish(&ctx, outcome)
}

pub async fn url_context(args: UrlContextArgs, ctx: ToolCtx) -> Result<ToolResult> {
    check_urls(&args.urls, 1)?;
    let host = ctx.host().clone();
    let loaded = load_config(&host, Some(ctx.snapshot())).await;
    let plan = plan::url_context(&host, ctx.snapshot(), &loaded).await;
    let call = call_for(&ctx);
    let count = args.urls.len();
    call.progress(&format!(
        "Analyzing {count} URL{}...",
        if count > 1 { "s" } else { "" }
    ));
    let outcome = match plan {
        Plan::Fail(result) => return Ok(result),
        Plan::Native(target) => native::url_context(&call, &target, &args.query, &args.urls)
            .await
            .map(|result| format_url_context(&result, &target.model.id)),
        Plan::Api(endpoint) => api::url_context(&call, &endpoint, &args.query, &args.urls)
            .await
            .map(|result| format_url_context(&result, endpoint.api.id())),
    };
    finish(&ctx, outcome)
}
