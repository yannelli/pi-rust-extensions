//! Web search and URL context tools for pi: provider-native search with the current model's
//! credentials, plus Exa, Firecrawl, Brave, and Tavily.

pub mod api;
pub mod auth;
pub mod citations;
pub mod config;
pub mod context;
pub mod format;
pub mod http;
pub mod keys;
pub mod model;
pub mod native;
pub mod pages;
pub mod plan;
pub mod results;
pub mod tools;
pub mod toolsync;
mod tui;
pub mod urls;

use std::process::ExitCode;

use pi_ext_sdk::prelude::*;

use crate::model::SearchModel;
use crate::toolsync::{Shared, sync};

async fn sync_event(state: Shared, ev: EventCtx) -> Result<()> {
    let model = if ev.name() == "model_select" {
        ev.get("model")
            .cloned()
            .and_then(SearchModel::from_json)
            .filter(|m| !m.base_url.is_empty())
    } else {
        None
    };
    if let Err(err) = sync(&state, ev.host(), ev.snapshot(), model).await {
        pi_ext_sdk::warn!("web-search: tool sync failed: {}", err.message());
    }
    Ok(())
}

pub fn extension() -> Extension {
    let state = Shared::default();
    let mut ext = Extension::new(config::NAME)
        .tool(tools::web_search_spec(), tools::web_search)
        .tool(tools::url_context_spec(), tools::url_context);
    for event in ["session_start", "session_tree", "model_select"] {
        let state = state.clone();
        ext = ext.on(event, move |ev: EventCtx| sync_event(state.clone(), ev));
    }
    ext.command(
        "web-search",
        "Choose web search providers, the search model, and API keys",
        move |_args, ctx: CommandCtx| tui::run(ctx.host().clone(), state.clone()),
    )
}

pub fn main() -> ExitCode {
    extension().run()
}
