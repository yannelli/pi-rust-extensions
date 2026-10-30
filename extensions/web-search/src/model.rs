//! The full pi model (with `baseUrl`, `headers`, `compat`) and its provider-native search kind.

use std::collections::BTreeMap;

use reqwest::Url;
use serde::Deserialize;
use serde_json::Value;

pub use crate::auth::{Auth, Target, env_api_key};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Google,
    OpenAi,
    Xai,
    Anthropic,
    DeepSeek,
    Ollama,
    Unsupported,
}

impl Kind {
    pub fn id(self) -> &'static str {
        match self {
            Kind::Google => "google",
            Kind::OpenAi => "openai",
            Kind::Xai => "xai",
            Kind::Anthropic => "anthropic",
            Kind::DeepSeek => "deepseek",
            Kind::Ollama => "ollama",
            Kind::Unsupported => "unsupported",
        }
    }
}

pub const SUPPORTED_PROVIDERS: [&str; 10] = [
    "deepseek",
    "google-generative-ai",
    "antigravity",
    "google-gemini-cli",
    "xai",
    "openai-responses",
    "azure-openai-responses",
    "openai-codex-responses",
    "anthropic-messages",
    "ollama",
];

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchModel {
    pub id: String,
    pub provider: String,
    pub api: String,
    pub base_url: String,
    pub reasoning: bool,
    pub max_tokens: u64,
    pub headers: BTreeMap<String, String>,
    pub compat: Value,
    pub thinking_level_map: Option<BTreeMap<String, Option<String>>>,
    #[serde(skip)]
    pub raw: Value,
}

impl SearchModel {
    pub fn from_json(raw: Value) -> Option<Self> {
        let mut model: SearchModel = serde_json::from_value(raw.clone()).ok()?;
        if model.id.is_empty() || model.provider.is_empty() {
            return None;
        }
        model.raw = raw;
        Some(model)
    }

    pub fn describe(&self) -> String {
        format!("{} ({}/{})", self.id, self.provider, self.api)
    }

    fn host(&self) -> Option<String> {
        Url::parse(&self.base_url)
            .ok()?
            .host_str()
            .map(str::to_owned)
    }

    pub fn kind(&self) -> Kind {
        let api = self.api.as_str();
        if self.provider == "deepseek" {
            Kind::DeepSeek
        } else if api == "google-generative-ai"
            || self.provider == "google-generative-ai"
            || crate::native::cloudcode::flavor(self).is_some()
        {
            Kind::Google
        } else if self.provider == "ollama-cloud" || self.host().as_deref() == Some("ollama.com") {
            Kind::Ollama
        } else if self.provider == "xai" && api == "openai-responses" {
            Kind::Xai
        } else if matches!(
            api,
            "openai-responses" | "azure-openai-responses" | "openai-codex-responses"
        ) {
            Kind::OpenAi
        } else if api == "anthropic-messages" {
            Kind::Anthropic
        } else {
            Kind::Unsupported
        }
    }

    pub fn supports_url_context(&self) -> bool {
        matches!(self.kind(), Kind::Google | Kind::Ollama)
    }

    pub fn is_codex(&self) -> bool {
        self.api == "openai-codex-responses"
    }

    /// Grok wire shape: `compat.webSearchDialect`, else xAI or a `grok` model id.
    pub fn is_grok_dialect(&self) -> bool {
        match self.compat["webSearchDialect"].as_str() {
            Some("grok") => return true,
            Some("openai") => return false,
            _ => {}
        }
        let id = self.id.to_ascii_lowercase();
        self.kind() == Kind::Xai
            || id == "grok"
            || id.starts_with("grok-")
            || id.starts_with("grok.")
    }

    pub fn is_opencode(&self) -> bool {
        matches!(self.provider.as_str(), "opencode" | "opencode-go")
            || self.host().as_deref() == Some("opencode.ai")
    }

    fn mapped(&self, level: &str) -> Option<Option<String>> {
        self.thinking_level_map.as_ref()?.get(level).cloned()
    }

    fn supports_level(&self, level: &str) -> bool {
        match self.mapped(level) {
            Some(None) => false,
            Some(Some(_)) => true,
            None => !matches!(level, "xhigh" | "max"),
        }
    }

    /// OpenAI `reasoning.effort` for the session level: clamped to the nearest supported
    /// level (higher first), mapped through `thinkingLevelMap`; `None` when off.
    pub fn reasoning_effort(&self, level: Option<&str>) -> Option<String> {
        const ALL: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
        let level = level.filter(|l| *l != "off")?;
        if !self.reasoning {
            return None;
        }
        let index = ALL.iter().position(|l| *l == level)?;
        let higher = ALL[index..].iter();
        let lower = ALL[..index].iter().rev();
        let clamped = higher
            .chain(lower)
            .find(|l| self.supports_level(l))
            .copied()?;
        if clamped == "off" {
            return None;
        }
        Some(
            self.mapped(clamped)
                .flatten()
                .unwrap_or_else(|| clamped.to_owned()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn model(value: Value) -> SearchModel {
        SearchModel::from_json(value).unwrap()
    }

    #[test]
    fn kinds_follow_provider_and_api() {
        let m = |provider: &str, api: &str, url: &str| {
            model(json!({ "id": "m", "provider": provider, "api": api, "baseUrl": url })).kind()
        };
        assert_eq!(m("deepseek", "openai-completions", ""), Kind::DeepSeek);
        assert_eq!(m("google", "google-generative-ai", ""), Kind::Google);
        assert_eq!(
            m("custom", "openai-completions", "https://ollama.com/v1"),
            Kind::Ollama
        );
        assert_eq!(m("xai", "openai-responses", ""), Kind::Xai);
        assert_eq!(
            m("openai-codex", "openai-codex-responses", ""),
            Kind::OpenAi
        );
        assert_eq!(m("anthropic", "anthropic-messages", ""), Kind::Anthropic);
        assert_eq!(m("groq", "openai-completions", ""), Kind::Unsupported);
    }

    #[test]
    fn reasoning_effort_clamps_and_maps() {
        let m = model(json!({
            "id": "gpt", "provider": "openai", "api": "openai-responses", "reasoning": true,
            "thinkingLevelMap": { "minimal": null, "xhigh": "x-high" }
        }));
        assert_eq!(m.reasoning_effort(Some("off")), None);
        assert_eq!(m.reasoning_effort(Some("minimal")).as_deref(), Some("low"));
        assert_eq!(m.reasoning_effort(Some("xhigh")).as_deref(), Some("x-high"));
        assert_eq!(m.reasoning_effort(Some("max")).as_deref(), Some("x-high"));
    }

    #[test]
    fn grok_dialect_detection() {
        let m = |id: &str, compat: Value| {
            model(
                json!({ "id": id, "provider": "gw", "api": "openai-responses", "compat": compat }),
            )
        };
        assert!(m("grok-4.6", json!(null)).is_grok_dialect());
        assert!(!m("grok-4.6", json!({ "webSearchDialect": "openai" })).is_grok_dialect());
        assert!(m("my-model", json!({ "webSearchDialect": "grok" })).is_grok_dialect());
        assert!(!m("grokker", json!(null)).is_grok_dialect());
    }
}
