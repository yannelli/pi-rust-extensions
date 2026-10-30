//! Turns `/api/tags` and `/api/show` into pi model definitions for provider `ollama`.

use serde_json::{Value, json};

use crate::client::{Client, Show, Tag};

/// pi's default for a model without a known context length.
pub const FALLBACK_CONTEXT: u64 = 128_000;
const MAX_OUTPUT: u64 = 16_384;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelInfo {
    pub tag: Tag,
    pub capabilities: Vec<String>,
    pub context_length: Option<u64>,
}

impl ModelInfo {
    pub fn new(tag: Tag, show: Option<Show>) -> Self {
        let show = show.unwrap_or_default();
        Self {
            context_length: show.context_length(),
            capabilities: show.capabilities,
            tag,
        }
    }

    pub fn has(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }

    /// Embedding-only models cannot chat.
    pub fn can_chat(&self) -> bool {
        self.capabilities.is_empty() || self.has("completion")
    }

    pub fn definition(&self) -> Value {
        let context = self.context_length.unwrap_or(FALLBACK_CONTEXT);
        let input = if self.has("vision") {
            json!(["text", "image"])
        } else {
            json!(["text"])
        };
        json!({
            "id": self.tag.name,
            "name": self.tag.name,
            "reasoning": self.has("thinking"),
            "input": input,
            "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
            "contextWindow": context,
            "maxTokens": context.min(MAX_OUTPUT),
        })
    }

    /// Lines for the details view.
    pub fn details(&self) -> Vec<String> {
        let d = &self.tag.details;
        let context = self
            .context_length
            .map_or("unknown".to_string(), |n| n.to_string());
        let capabilities = if self.capabilities.is_empty() {
            "unknown".into()
        } else {
            self.capabilities.join(", ")
        };
        vec![
            format!("Model:          {}", self.tag.name),
            format!("Family:         {}", d.family),
            format!("Parameters:     {}", d.parameter_size),
            format!("Quantization:   {}", d.quantization_level),
            format!("Size:           {}", human_size(self.tag.size)),
            format!("Context length: {context}"),
            format!("Capabilities:   {capabilities}"),
            format!("Modified:       {}", self.tag.modified_at),
        ]
    }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Lists the installed models with their `/api/show` data.
pub async fn discover(client: &Client) -> Result<Vec<ModelInfo>, String> {
    let mut models = Vec::new();
    for tag in client.tags().await? {
        let show = client.show(&tag.name).await.ok();
        models.push(ModelInfo::new(tag, show));
    }
    models.sort_by(|a, b| a.tag.name.cmp(&b.tag.name));
    Ok(models)
}

/// The `provider.register` config: Ollama's OpenAI-compatible API at `<host>/v1`.
pub fn provider_config(base: &str, models: &[ModelInfo]) -> Value {
    let models: Vec<Value> = models
        .iter()
        .filter(|m| m.can_chat())
        .map(ModelInfo::definition)
        .collect();
    json!({
        "baseUrl": format!("{}/v1", base.trim_end_matches('/')),
        "api": "openai-completions",
        "apiKey": "ollama",
        "models": models,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(name: &str, capabilities: &[&str], context: Option<u64>) -> ModelInfo {
        ModelInfo {
            tag: Tag {
                name: name.into(),
                ..Default::default()
            },
            capabilities: capabilities.iter().map(|c| c.to_string()).collect(),
            context_length: context,
        }
    }

    #[test]
    fn provider_config_lists_chat_models() {
        let models = [
            model(
                "qwen3:8b",
                &["completion", "tools", "thinking"],
                Some(40960),
            ),
            model("llava:7b", &["completion", "vision"], None),
            model("nomic-embed-text", &["embedding"], Some(2048)),
        ];
        let config = provider_config("http://127.0.0.1:11434", &models);
        assert_eq!(config["baseUrl"], "http://127.0.0.1:11434/v1");
        assert_eq!(config["api"], "openai-completions");
        let list = config["models"].as_array().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["reasoning"], true);
        assert_eq!(list[0]["contextWindow"], 40960);
        assert_eq!(list[0]["maxTokens"], 16384);
        assert_eq!(list[1]["input"], json!(["text", "image"]));
        assert_eq!(list[1]["contextWindow"], FALLBACK_CONTEXT);
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(4_683_087_332), "4.7 GB");
    }
}
