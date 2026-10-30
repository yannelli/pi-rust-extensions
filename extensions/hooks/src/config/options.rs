use std::path::Path;

use serde_json::{Map, Value};

use crate::model::{Scope, Source};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeOption {
    Global,
    Project,
    Both,
}

impl ScopeOption {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "global" => Some(Self::Global),
            "project" => Some(Self::Project),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
            Self::Both => "both",
        }
    }

    pub fn allows(self, scope: Scope) -> bool {
        matches!(
            (self, scope),
            (Self::Both, _) | (Self::Global, Scope::Global) | (Self::Project, Scope::Project)
        )
    }
}

/// Options from `ext/hooks.json`: global file first, project file wins per key.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub claude: bool,
    pub codex: bool,
    pub pi: bool,
    pub scope: ScopeOption,
    pub trust_all: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            claude: true,
            codex: true,
            pi: true,
            scope: ScopeOption::Both,
            trust_all: false,
        }
    }
}

impl Options {
    pub fn merge(layers: &[&Value]) -> Self {
        let mut options = Self::default();
        for layer in layers {
            if let Some(sources) = layer.get("sources").and_then(Value::as_object) {
                for source in Source::ALL {
                    if let Some(on) = sources.get(source.name()).and_then(Value::as_bool) {
                        *options.source_mut(source) = on;
                    }
                }
            }
            if let Some(scope) = layer.get("scope").and_then(Value::as_str) {
                options.scope = ScopeOption::parse(scope).unwrap_or(options.scope);
            }
            if let Some(trust_all) = layer.get("trustAll").and_then(Value::as_bool) {
                options.trust_all = trust_all;
            }
        }
        options
    }

    pub fn source_on(&self, source: Source) -> bool {
        match source {
            Source::Claude => self.claude,
            Source::Codex => self.codex,
            Source::Pi => self.pi,
        }
    }

    pub fn source_mut(&mut self, source: Source) -> &mut bool {
        match source {
            Source::Claude => &mut self.claude,
            Source::Codex => &mut self.codex,
            Source::Pi => &mut self.pi,
        }
    }
}

pub fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(value) => Some(value),
        Err(err) => {
            pi_ext_sdk::warn!("hooks: cannot parse {}: {err}", path.display());
            None
        }
    }
}

/// Sets `key` (a `/`-separated path) in a JSON file and keeps every other key.
pub fn write_option(path: &Path, key: &str, value: Value) -> std::io::Result<()> {
    let mut doc = read_json(path)
        .filter(Value::is_object)
        .unwrap_or(Value::Object(Map::new()));
    let mut target = &mut doc;
    let parts: Vec<&str> = key.split('/').collect();
    for part in &parts[..parts.len() - 1] {
        let object = target.as_object_mut().expect("object");
        let entry = object
            .entry(part.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        target = entry;
    }
    let last = parts[parts.len() - 1].to_owned();
    target.as_object_mut().expect("object").insert(last, value);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(&doc).map_err(std::io::Error::other)?;
    std::fs::write(path, text + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn project_wins_per_key() {
        let global = json!({ "sources": { "claude": false, "codex": false }, "trustAll": true });
        let project = json!({ "sources": { "codex": true }, "scope": "project" });
        let options = Options::merge(&[&global, &project]);
        assert!(!options.claude && options.codex && options.pi);
        assert_eq!(options.scope, ScopeOption::Project);
        assert!(options.trust_all);
        assert_eq!(Options::merge(&[]), Options::default());
    }

    #[test]
    fn write_option_keeps_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ext/hooks.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"hooks":{"Stop":[]},"x":1}"#).unwrap();
        write_option(&path, "sources/codex", json!(false)).unwrap();
        write_option(&path, "trustAll", json!(true)).unwrap();
        let doc = read_json(&path).unwrap();
        assert_eq!(
            doc,
            json!({ "hooks": { "Stop": [] }, "x": 1, "sources": { "codex": false }, "trustAll": true })
        );
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("\n  \"x\": 1")
        );
    }
}
