//! Model credentials and the per-call native search target.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::model::SearchModel;

/// Credentials from `ctx.modelRegistry.getApiKeyAndHeaders`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Auth {
    pub api_key: Option<String>,
    pub headers: BTreeMap<String, String>,
}

impl Auth {
    pub fn from_json(value: &Value) -> Result<Auth, String> {
        if value["ok"] == Value::Bool(false) {
            let error = value["error"]
                .as_str()
                .unwrap_or("Failed to get API key and headers");
            return Err(error.to_owned());
        }
        let api_key = value["apiKey"]
            .as_str()
            .filter(|k| !k.is_empty())
            .map(str::to_owned);
        let headers = value["headers"]
            .as_object()
            .map(|map| {
                let pairs = map
                    .iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())));
                pairs.collect()
            })
            .unwrap_or_default();
        Ok(Auth { api_key, headers })
    }

    pub fn has_auth_header(&self) -> bool {
        self.headers.iter().any(|(name, value)| {
            let name = name.to_ascii_lowercase();
            !value.is_empty()
                && matches!(
                    name.as_str(),
                    "authorization" | "x-api-key" | "x-goog-api-key"
                )
        })
    }
}

/// Env fallback for providers whose key pi resolves only inside its own stream path.
pub fn env_api_key(provider: &str) -> Option<String> {
    let names: &[&str] = match provider {
        "openai" => &["OPENAI_API_KEY"],
        "anthropic" => &["ANTHROPIC_API_KEY"],
        "google" | "google-generative-ai" => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        "xai" => &["XAI_API_KEY"],
        "deepseek" => &["DEEPSEEK_API_KEY"],
        "ollama-cloud" | "ollama" => &["OLLAMA_API_KEY"],
        "azure-openai-responses" => &["AZURE_OPENAI_API_KEY"],
        "opencode" | "opencode-go" => &["OPENCODE_API_KEY"],
        _ => &[],
    };
    names
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|v| !v.is_empty()))
}

/// What a native provider call needs besides the prompt.
#[derive(Debug, Clone, Default)]
pub struct Target {
    pub model: SearchModel,
    pub auth: Auth,
    pub session_id: Option<String>,
    pub thinking_level: Option<String>,
}

impl Target {
    /// `x-opencode-*` headers that OpenCode Zen and Go require per conversation.
    pub fn session_headers(&self) -> Vec<(String, String)> {
        match &self.session_id {
            Some(id) if self.model.is_opencode() => vec![
                ("x-opencode-session".to_owned(), id.clone()),
                ("x-opencode-client".to_owned(), "pi".to_owned()),
            ],
            _ => Vec::new(),
        }
    }
}
