//! Config: `<agentDir>/ext/todo.json` and `<cwd>/.pi/ext/todo.json`; project wins per key.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::scopes::Visibility;

pub const DEFAULT_MAX_WIDGET_LINES: usize = 12;
pub const DEFAULT_COLLAPSE_KEY: &str = "ctrl+shift+t";
pub const COLLAPSE_KEY_OFF: &str = "off";

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub max_widget_lines: usize,
    pub collapse_key: String,
    pub default_visibility: Visibility,
    pub prompt_snippet: Option<String>,
    pub prompt_guidelines: Option<Vec<String>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_widget_lines: DEFAULT_MAX_WIDGET_LINES,
            collapse_key: DEFAULT_COLLAPSE_KEY.into(),
            default_visibility: Visibility::default(),
            prompt_snippet: None,
            prompt_guidelines: None,
        }
    }
}

pub fn paths(agent_dir: &Path, cwd: &Path) -> [PathBuf; 2] {
    [
        agent_dir.join("ext").join("todo.json"),
        cwd.join(".pi").join("ext").join("todo.json"),
    ]
}

fn read(path: &Path) -> Map<String, Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Map::new();
    };
    match serde_json::from_str(&text) {
        Ok(Value::Object(map)) => map,
        _ => {
            pi_ext_sdk::warn!("ignoring unreadable todo config {}", path.display());
            Map::new()
        }
    }
}

/// Merges config objects in order; later objects win per key, `guidance` per field.
pub fn merge(layers: &[Map<String, Value>]) -> Map<String, Value> {
    let mut merged = Map::new();
    for layer in layers {
        for (key, value) in layer {
            match (merged.get_mut(key), value) {
                (Some(Value::Object(base)), Value::Object(over)) if key == "guidance" => {
                    base.extend(over.clone());
                }
                _ => {
                    merged.insert(key.clone(), value.clone());
                }
            }
        }
    }
    merged
}

pub fn load(agent_dir: &Path, cwd: &Path) -> Config {
    let layers = paths(agent_dir, cwd).map(|p| read(&p));
    from_map(&merge(&layers))
}

pub fn from_map(map: &Map<String, Value>) -> Config {
    let mut config = Config::default();
    if let Some(n) = map
        .get("maxWidgetLines")
        .and_then(Value::as_u64)
        .filter(|n| *n >= 3)
    {
        config.max_widget_lines = n as usize;
    }
    if let Some(key) = map
        .get("collapseKey")
        .and_then(Value::as_str)
        .map(str::trim)
    {
        if key == COLLAPSE_KEY_OFF || valid_key(key) {
            config.collapse_key = key.to_owned();
        } else if !key.is_empty() {
            pi_ext_sdk::warn!("invalid collapseKey \"{key}\"; using {DEFAULT_COLLAPSE_KEY}");
        }
    }
    if let Some(v) = map.get("defaultVisibility") {
        config.default_visibility = serde_json::from_value(v.clone()).unwrap_or_default();
    }
    let guidance = map.get("guidance").and_then(Value::as_object);
    config.prompt_snippet = guidance
        .and_then(|g| g.get("promptSnippet"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned);
    config.prompt_guidelines = guidance
        .and_then(|g| g.get("promptGuidelines"))
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|list| !list.is_empty());
    config
}

const SPECIAL_KEYS: &[&str] = &[
    "escape",
    "esc",
    "enter",
    "return",
    "tab",
    "space",
    "backspace",
    "delete",
    "insert",
    "clear",
    "home",
    "end",
    "pageup",
    "pagedown",
    "up",
    "down",
    "left",
    "right",
];

/// pi-tui KeyId grammar: `modifier+...+key`.
pub fn valid_key(spec: &str) -> bool {
    if spec.is_empty() || spec.starts_with('+') || spec.ends_with('+') || spec.contains("++") {
        return false;
    }
    let lower = spec.to_ascii_lowercase();
    let parts: Vec<&str> = lower.split('+').collect();
    let (base, modifiers) = parts.split_last().expect("non-empty");
    let mut seen = Vec::new();
    for m in modifiers {
        if !["ctrl", "shift", "alt", "super"].contains(m) || seen.contains(m) {
            return false;
        }
        seen.push(*m);
    }
    let function_key = base
        .strip_prefix('f')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=12).contains(&n));
    base.chars().count() == 1 || SPECIAL_KEYS.contains(base) || function_key
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn project_wins_per_key_and_guidance_per_field() {
        let global = map(json!({ "maxWidgetLines": 8, "collapseKey": "alt+o",
            "guidance": { "promptSnippet": "global", "promptGuidelines": ["g"] } }));
        let project = map(json!({ "maxWidgetLines": 5, "defaultVisibility": "all",
            "guidance": { "promptSnippet": "project" } }));
        let config = from_map(&merge(&[global, project]));
        assert_eq!(config.max_widget_lines, 5);
        assert_eq!(config.collapse_key, "alt+o");
        assert_eq!(config.default_visibility, Visibility::All);
        assert_eq!(config.prompt_snippet.as_deref(), Some("project"));
        assert_eq!(config.prompt_guidelines, Some(vec!["g".to_string()]));
    }

    #[test]
    fn invalid_values_fall_back_to_defaults() {
        let config = from_map(&map(json!({ "maxWidgetLines": 2, "collapseKey": "hyper+x",
            "defaultVisibility": "loud" })));
        assert_eq!(config, Config::default());
        assert_eq!(
            from_map(&map(json!({ "collapseKey": "off" }))).collapse_key,
            "off"
        );
    }

    #[test]
    fn key_grammar() {
        assert!(valid_key("ctrl+shift+t"));
        assert!(valid_key("ctrl+]"));
        assert!(valid_key("f5"));
        assert!(!valid_key("ctrl+ctrl+t"));
        assert!(!valid_key("ctrl+"));
        assert!(!valid_key("meta+x"));
    }

    #[test]
    fn loads_files_from_both_paths() {
        let dir = pi_ext_e2e::TempDir::new().unwrap();
        let (agent, cwd) = (dir.path().join("agent"), dir.path().join("proj"));
        let [global, project] = paths(&agent, &cwd);
        for (path, text) in [
            (&global, r#"{"maxWidgetLines": 9}"#),
            (&project, r#"{"collapseKey": "alt+t"}"#),
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let config = load(&agent, &cwd);
        assert_eq!(
            (config.max_widget_lines, config.collapse_key.as_str()),
            (9, "alt+t")
        );
    }
}
