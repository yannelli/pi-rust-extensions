use std::path::Path;

use serde_json::{Map, Value};
use toml_edit::{DocumentMut, Item, Table};

use super::options::read_json;

/// One Codex layer (`hooks.json` plus inline `config.toml` hooks).
#[derive(Debug, Default)]
pub struct CodexLayer {
    /// `(file, hooks object)` pairs.
    pub hooks: Vec<(std::path::PathBuf, Value)>,
    /// `[features] hooks` (or `codex_hooks`) when set.
    pub feature: Option<bool>,
}

pub fn load_layer(dir: &Path) -> CodexLayer {
    let mut layer = CodexLayer::default();
    let json_path = dir.join("hooks.json");
    if let Some(hooks) = read_json(&json_path).and_then(|doc| doc.get("hooks").cloned()) {
        layer.hooks.push((json_path, hooks));
    }
    let toml_path = dir.join("config.toml");
    if let Some(doc) = read_toml(&toml_path) {
        let features = doc.get("features");
        layer.feature = features
            .and_then(|f| f.get("hooks").or_else(|| f.get("codex_hooks")))
            .and_then(Value::as_bool);
        if let Some(hooks) = doc
            .get("hooks")
            .filter(|h| h.as_object().is_some_and(|o| !o.is_empty()))
        {
            if !layer.hooks.is_empty() {
                pi_ext_sdk::warn!(
                    "hooks: {} has hooks in both hooks.json and config.toml; merging them",
                    dir.display()
                );
            }
            layer.hooks.push((toml_path, hooks.clone()));
        }
    }
    layer
}

pub fn read_toml(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    match text.parse::<DocumentMut>() {
        Ok(doc) => Some(table_to_json(doc.as_table())),
        Err(err) => {
            pi_ext_sdk::warn!("hooks: cannot parse {}: {err}", path.display());
            None
        }
    }
}

pub fn table_to_json(table: &Table) -> Value {
    let map: Map<String, Value> = table
        .iter()
        .filter_map(|(key, item)| item_to_json(item).map(|v| (key.to_owned(), v)))
        .collect();
    Value::Object(map)
}

fn item_to_json(item: &Item) -> Option<Value> {
    match item {
        Item::None => None,
        Item::Value(value) => Some(value_to_json(value)),
        Item::Table(table) => Some(table_to_json(table)),
        Item::ArrayOfTables(array) => Some(Value::Array(array.iter().map(table_to_json).collect())),
    }
}

fn value_to_json(value: &toml_edit::Value) -> Value {
    use toml_edit::Value as T;
    match value {
        T::String(s) => Value::String(s.value().clone()),
        T::Integer(i) => Value::from(*i.value()),
        T::Float(f) => serde_json::Number::from_f64(*f.value()).map_or(Value::Null, Value::Number),
        T::Boolean(b) => Value::Bool(*b.value()),
        T::Datetime(d) => Value::String(d.value().to_string()),
        T::Array(array) => Value::Array(array.iter().map(value_to_json).collect()),
        T::InlineTable(table) => Value::Object(
            table
                .iter()
                .map(|(key, v)| (key.to_owned(), value_to_json(v)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inline_toml_hooks_convert() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            r#"
model = "o3"
[features]
codex_hooks = false

[[hooks.PreToolUse]]
matcher = "^Bash$"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "./check.sh"
timeout = 5
"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("hooks.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"s"}]}]}}"#,
        )
        .unwrap();
        let layer = load_layer(dir.path());
        assert_eq!(layer.feature, Some(false));
        assert_eq!(layer.hooks.len(), 2);
        assert_eq!(
            layer.hooks[1].1,
            json!({ "PreToolUse": [{ "matcher": "^Bash$", "hooks": [{ "type": "command", "command": "./check.sh", "timeout": 5 }] }] })
        );
    }
}
