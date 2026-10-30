use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// The ctx snapshot pi sends with `event`, `tool.execute`, `command.run`, and `shortcut.run`.
// `remote = "Self"` turns the derives into inherent functions; the trait impls below wrap them.
// Inside this crate call `<Snapshot as Deserialize>::deserialize` to fill `is_project_trusted`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", rename_all = "camelCase", default)]
pub struct Snapshot {
    pub cwd: String,
    pub mode: Option<String>,
    #[serde(rename = "hasUI")]
    pub has_ui: bool,
    pub model: Option<Model>,
    /// The `--models` scope; empty when there is none.
    pub scoped_models: Vec<ScopedModel>,
    pub thinking_level: Option<String>,
    pub session_id: Option<String>,
    pub session_name: Option<String>,
    pub session_file: Option<String>,
    pub is_idle: Option<bool>,
    pub context_usage: Option<Value>,
    /// Terminal columns in `tui` mode, else `None`.
    pub terminal_width: Option<u64>,
    pub flags: Map<String, Value>,
    pub footer: Option<Value>,
    /// `isProjectTrusted`, read while parsing; the key also stays in `extra`.
    #[serde(skip)]
    pub is_project_trusted: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl<'de> Deserialize<'de> for Snapshot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut snapshot = Snapshot::deserialize(deserializer)?;
        let trusted = snapshot
            .extra
            .get("isProjectTrusted")
            .and_then(Value::as_bool);
        snapshot.is_project_trusted = trusted.unwrap_or(false);
        Ok(snapshot)
    }
}

impl Serialize for Snapshot {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Snapshot::serialize(self, serializer)
    }
}

/// One entry of `Snapshot::scoped_models`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScopedModel {
    pub provider: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
}

/// `contextUsage` of the snapshot and of `ctx.getContextUsage`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContextUsage {
    pub tokens: Option<u64>,
    pub context_window: u64,
    pub percent: Option<f64>,
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
        <Self as Deserialize>::deserialize(value).unwrap_or_else(|err| {
            crate::warn!("unreadable ctx snapshot: {err}");
            Self::default()
        })
    }

    /// `contextUsage` as a typed value; `None` when absent or unreadable.
    pub fn context_usage(&self) -> Option<ContextUsage> {
        let usage = self
            .context_usage
            .as_ref()
            .filter(|usage| !usage.is_null())?;
        ContextUsage::deserialize(usage).ok()
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
    pub base_url: Option<String>,
    /// The model's `compat` options; pi sends no headers and no keys.
    pub compat: Option<Value>,
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
        assert!(!snapshot.is_project_trusted);
        assert!(snapshot.scoped_models.is_empty());
        assert_eq!(snapshot.terminal_width, None);
    }

    #[test]
    fn project_trust_is_typed_and_stays_in_extra() {
        let snapshot: Snapshot =
            serde_json::from_value(json!({ "cwd": "/work", "isProjectTrusted": true })).unwrap();
        assert!(snapshot.is_project_trusted);
        assert_eq!(snapshot.extra.get("isProjectTrusted"), Some(&json!(true)));
        let round_trip = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(round_trip["isProjectTrusted"], json!(true));
        assert!(Snapshot::deserialize_lenient(&round_trip).is_project_trusted);
    }

    #[test]
    fn parses_scoped_models_terminal_width_and_model_endpoint() {
        let snapshot: Snapshot = serde_json::from_value(json!({
            "cwd": "/work", "terminalWidth": 120,
            "scopedModels": [{ "provider": "local", "id": "other", "thinkingLevel": "high" }, { "provider": "a", "id": "b" }],
            "model": { "provider": "local", "id": "tiny", "baseUrl": "http://x", "compat": { "supportsStore": false } }
        }))
        .unwrap();
        assert_eq!(snapshot.terminal_width, Some(120));
        assert_eq!(
            snapshot.scoped_models[0].thinking_level.as_deref(),
            Some("high")
        );
        assert_eq!(
            snapshot.scoped_models[1],
            ScopedModel {
                provider: "a".into(),
                id: "b".into(),
                thinking_level: None
            }
        );
        let model = snapshot.model.unwrap();
        assert_eq!(model.base_url.as_deref(), Some("http://x"));
        assert_eq!(model.compat, Some(json!({ "supportsStore": false })));
        assert!(model.extra.is_empty());
    }

    #[test]
    fn context_usage_accessor() {
        let usage = json!({ "tokens": 1200, "contextWindow": 128000, "percent": 0.94 });
        let snapshot: Snapshot = serde_json::from_value(json!({ "contextUsage": usage })).unwrap();
        let typed = snapshot.context_usage().unwrap();
        assert_eq!(
            (typed.tokens, typed.context_window, typed.percent),
            (Some(1200), 128000, Some(0.94))
        );
        let unknown =
            json!({ "contextUsage": { "tokens": null, "contextWindow": 1000, "percent": null } });
        let snapshot: Snapshot = serde_json::from_value(unknown).unwrap();
        assert_eq!(snapshot.context_usage().unwrap().tokens, None);
        assert_eq!(Snapshot::default().context_usage(), None);
    }
}
