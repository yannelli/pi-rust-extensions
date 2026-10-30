//! Provider-native search with the credentials of the selected pi model.

pub mod anthropic;
mod anthropic_events;
pub mod cloudcode;
pub mod google;
pub mod ollama;
pub mod openai;
mod openai_events;

use anyhow::{Result, bail};

use crate::http::Call;
use crate::model::{Kind, Target};
use crate::results::StreamResult;

/// `web_search`: every provider except Ollama reads extra URLs from the prompt.
pub async fn web_search(
    call: &Call,
    target: &Target,
    query: &str,
    urls: &[String],
) -> Result<StreamResult> {
    let kind = target.model.kind();
    let prompt = if urls.is_empty() || kind == Kind::Ollama {
        query.to_owned()
    } else {
        format!("{query}\n\nAlso analyze these URLs:\n{}", urls.join("\n"))
    };
    match kind {
        Kind::Google => {
            google::generate(
                call,
                target,
                &google::web_search_body(&prompt, !urls.is_empty()),
            )
            .await
        }
        Kind::Ollama => ollama::search(call, target, query, urls).await,
        Kind::OpenAi | Kind::Xai => openai::search(call, target, &prompt).await,
        Kind::Anthropic | Kind::DeepSeek => anthropic::search(call, target, &prompt).await,
        Kind::Unsupported => {
            let model = &target.model;
            bail!(
                "Unsupported provider for web search: {} ({})",
                model.provider,
                model.api
            )
        }
    }
}

/// `url_context`: Gemini URL Context or Ollama web fetch.
pub async fn url_context(
    call: &Call,
    target: &Target,
    query: &str,
    urls: &[String],
) -> Result<StreamResult> {
    match target.model.kind() {
        Kind::Google => {
            let videos = target.model.api == "google-generative-ai"
                && cloudcode::flavor(&target.model).is_none();
            google::generate(call, target, &google::url_context_body(query, urls, videos)).await
        }
        Kind::Ollama => ollama::url_context(call, target, query, urls).await,
        _ => bail!("url_context needs a Google Gemini or Ollama Cloud model"),
    }
}
