use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// The ctx snapshot pi sends with `event`, `tool.execute`, `command.run`, and `shortcut.run`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub cwd: String,
    pub mode: Option<String>,
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
    /// `isProjectTrusted`; parsing also keeps the key in `extra`, and serializing writes this value.
    pub is_project_trusted: bool,
    pub extra: Map<String, Value>,
}

const PROJECT_TRUSTED: &str = "isProjectTrusted";

/// Wire form of `Snapshot`; `isProjectTrusted` travels in `extra`.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SnapshotWire {
    cwd: String,
    mode: Option<String>,
    #[serde(rename = "hasUI")]
    has_ui: bool,
    model: Option<Model>,
    scoped_models: Vec<ScopedModel>,
    thinking_level: Option<String>,
    session_id: Option<String>,
    session_name: Option<String>,
    session_file: Option<String>,
    is_idle: Option<bool>,
    context_usage: Option<Value>,
    terminal_width: Option<u64>,
    flags: Map<String, Value>,
    footer: Option<Value>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl<'de> Deserialize<'de> for Snapshot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = SnapshotWire::deserialize(deserializer)?;
        let is_project_trusted = wire
            .extra
            .get(PROJECT_TRUSTED)
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(Snapshot {
            cwd: wire.cwd,
            mode: wire.mode,
            has_ui: wire.has_ui,
            model: wire.model,
            scoped_models: wire.scoped_models,
            thinking_level: wire.thinking_level,
            session_id: wire.session_id,
            session_name: wire.session_name,
            session_file: wire.session_file,
            is_idle: wire.is_idle,
            context_usage: wire.context_usage,
            terminal_width: wire.terminal_width,
            flags: wire.flags,
            footer: wire.footer,
            is_project_trusted,
            extra: wire.extra,
        })
    }
}

impl Serialize for Snapshot {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut extra = self.extra.clone();
        if self.is_project_trusted || extra.contains_key(PROJECT_TRUSTED) {
            extra.insert(
                PROJECT_TRUSTED.to_owned(),
                Value::Bool(self.is_project_trusted),
            );
        }
        SnapshotWire {
            cwd: self.cwd.clone(),
            mode: self.mode.clone(),
            has_ui: self.has_ui,
            model: self.model.clone(),
            scoped_models: self.scoped_models.clone(),
            thinking_level: self.thinking_level.clone(),
            session_id: self.session_id.clone(),
            session_name: self.session_name.clone(),
            session_file: self.session_file.clone(),
            is_idle: self.is_idle,
            context_usage: self.context_usage.clone(),
            terminal_width: self.terminal_width,
            flags: self.flags.clone(),
            footer: self.footer.clone(),
            extra,
        }
        .serialize(serializer)
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
        Self::deserialize(value).unwrap_or_else(|err| {
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

    #[test]
    fn trait_deserialize_fills_project_trust() {
        let value = json!({ "cwd": "/work", "isProjectTrusted": true });
        let via_trait = Snapshot::deserialize(&value).unwrap();
        let via_qualified = <Snapshot as Deserialize>::deserialize(&value).unwrap();
        let via_from_value: Snapshot = serde_json::from_value(value.clone()).unwrap();
        for snapshot in [via_trait, via_qualified, via_from_value] {
            assert!(snapshot.is_project_trusted);
            assert_eq!(snapshot.extra.get("isProjectTrusted"), Some(&json!(true)));
        }
    }

    #[test]
    fn serialize_writes_typed_project_trust_once() {
        let snapshot = Snapshot {
            cwd: "/work".into(),
            is_project_trusted: true,
            ..Snapshot::default()
        };
        let text = serde_json::to_string(&snapshot).unwrap();
        assert_eq!(text.matches("\"isProjectTrusted\"").count(), 1);
        let parsed: Snapshot = serde_json::from_str(&text).unwrap();
        assert!(parsed.is_project_trusted);
        assert_eq!(parsed.cwd, "/work");

        let mut stale = parsed;
        stale.is_project_trusted = false;
        let text = serde_json::to_string(&stale).unwrap();
        assert_eq!(text.matches("\"isProjectTrusted\"").count(), 1);
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["isProjectTrusted"], json!(false));

        let untrusted = serde_json::to_value(Snapshot::default()).unwrap();
        assert!(untrusted.get("isProjectTrusted").is_none());
    }
}
