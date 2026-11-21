use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Map, Value};

use crate::model::{Scope, Source};
use crate::policy::ConflictMode;

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

/// An on/off row of the Settings tab. pi reads it from the global options file only.
#[derive(Debug, PartialEq, Eq)]
pub struct Switch {
    /// `/`-separated key in the options file, for example `managed/claudeMdm`.
    pub key: &'static str,
    pub label: &'static str,
    pub about: &'static str,
    pub default: bool,
}

/// The switches that features add, in Settings tab order.
pub const SWITCHES: &[Switch] = &[];

/// The value of each switch in `doc`, else its default.
pub fn read_switches(doc: &Value, switches: &'static [Switch]) -> BTreeMap<&'static str, bool> {
    switches
        .iter()
        .map(|switch| {
            let pointer = format!("/{}", switch.key);
            let value = doc.pointer(&pointer).and_then(Value::as_bool);
            (switch.key, value.unwrap_or(switch.default))
        })
        .collect()
}

/// Options from `ext/hooks.json`: global file first, project file wins per key.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub claude: bool,
    pub codex: bool,
    pub pi: bool,
    pub scope: ScopeOption,
    pub trust_all: bool,
    /// Read from the global options file only.
    pub conflicts: ConflictMode,
    /// Hashes from the `disabled` lists of both files.
    pub disabled: BTreeSet<String>,
    /// [`SWITCHES`] values from the global options file.
    pub switches: BTreeMap<&'static str, bool>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            claude: true,
            codex: true,
            pi: true,
            scope: ScopeOption::Both,
            trust_all: false,
            conflicts: ConflictMode::Merge,
            disabled: BTreeSet::new(),
            switches: BTreeMap::new(),
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
            if let Some(mode) = layer.get("conflicts").and_then(Value::as_str) {
                options.conflicts = ConflictMode::parse(mode).unwrap_or(options.conflicts);
            }
            if let Some(list) = layer.get("disabled").and_then(Value::as_array) {
                let hashes = list.iter().filter_map(Value::as_str).map(str::to_owned);
                options.disabled.extend(hashes);
            }
        }
        options
    }

    /// The value of the switch `key`; its default when the file does not set it.
    pub fn switch_on(&self, key: &str) -> bool {
        self.switches.get(key).copied().unwrap_or_else(|| {
            SWITCHES
                .iter()
                .find(|s| s.key == key)
                .is_none_or(|s| s.default)
        })
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
    fn switches_read_the_global_file_with_defaults() {
        static TEST: [Switch; 2] = [
            Switch {
                key: "managed/a",
                label: "A",
                about: "",
                default: true,
            },
            Switch {
                key: "limits/b",
                label: "B",
                about: "",
                default: false,
            },
        ];
        let doc = json!({ "managed": { "a": false } });
        let values = read_switches(&doc, &TEST);
        assert_eq!(values.get("managed/a"), Some(&false));
        assert_eq!(values.get("limits/b"), Some(&false));
        let doc = json!({ "limits": { "b": true } });
        assert_eq!(read_switches(&doc, &TEST).get("limits/b"), Some(&true));
        assert!(Options::default().switch_on("unknown"));
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
