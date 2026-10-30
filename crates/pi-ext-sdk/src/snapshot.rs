use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The ctx snapshot pi sends with `event`, `tool.execute`, `command.run`, and `shortcut.run`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Snapshot {
    pub cwd: String,
    pub mode: Option<String>,
    #[serde(rename = "hasUI")]
    pub has_ui: bool,
    pub model: Option<Model>,
    pub thinking_level: Option<String>,
    pub session_id: Option<String>,
    pub session_name: Option<String>,
    pub session_file: Option<String>,
    pub is_idle: Option<bool>,
    pub context_usage: Option<Value>,
    pub flags: Map<String, Value>,
    pub footer: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Snapshot {
    pub fn cwd_path(&self) -> PathBuf {
        PathBuf::from(&self.cwd)
    }

    pub fn flag(&self, name: &str) -> Option<&Value> {
        self.flags.get(name).filter(|value| !value.is_null())
    }

    pub fn flag_bool(&self, name: &str) -> bool {
        self.flag(name).and_then(Value::as_bool).unwrap_or(false)
    }

    pub fn flag_str(&self, name: &str) -> Option<&str> {
        self.flag(name).and_then(Value::as_str)
    }

    pub(crate) fn deserialize_lenient(value: &Value) -> Self {
        Self::deserialize(value).unwrap_or_else(|err| {
            crate::warn!("unreadable ctx snapshot: {err}");
            Self::default()
        })
    }

    /// `provider/id` of the current model.
    pub fn model_ref(&self) -> Option<String> {
        self.model
            .as_ref()
            .map(|model| format!("{}/{}", model.provider, model.id))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Model {
    pub provider: String,
    pub id: String,
    pub name: Option<String>,
    pub api: Option<String>,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
    pub reasoning: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Params of `initialize`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InitParams {
    pub protocol_version: i64,
    pub pi_version: String,
    pub cwd: String,
    pub agent_dir: String,
    pub extension_path: String,
    pub platform: String,
    pub mode: String,
    #[serde(rename = "hasUI")]
    pub has_ui: bool,
    pub flags: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_partial_snapshot() {
        let snapshot: Snapshot = serde_json::from_value(json!({
            "cwd": "/work", "hasUI": true, "flags": { "plan": true },
            "model": { "provider": "openai", "id": "gpt-5", "contextWindow": 400000 },
            "newField": 1
        }))
        .unwrap();
        assert!(snapshot.has_ui);
        assert!(snapshot.flag_bool("plan"));
        assert_eq!(snapshot.model_ref().as_deref(), Some("openai/gpt-5"));
        assert_eq!(snapshot.extra.get("newField"), Some(&json!(1)));
    }
}
