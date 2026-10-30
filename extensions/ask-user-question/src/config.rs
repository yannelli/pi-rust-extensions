//! Config files and key specs (`collapseKey`, `notesKey`).

use std::path::{Path, PathBuf};

use pi_ext_sdk::Key;
use serde_json::{Map, Value};

pub const DEFAULT_COLLAPSE_KEY: &str = "ctrl+]";
pub const COLLAPSE_KEY_OFF: &str = "off";
pub const DEFAULT_NOTES_KEY: &str = "n";

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
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
];
const MODIFIERS: &[&str] = &["ctrl", "shift", "alt", "super"];
const PRINTABLE: &str = "abcdefghijklmnopqrstuvwxyz0123456789_-!@#$%^&*()|~`'\":;,./<>?[]{}=\\";
const NOTES_KEY_RESERVED: &[&str] = &[
    "space", "enter", "return", "tab", "escape", "esc", "up", "down", "left", "right",
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Guidance {
    pub description: Option<String>,
    pub prompt_snippet: Option<String>,
    pub prompt_guidelines: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub collapse_key: String,
    pub notes_key: String,
    pub guidance: Guidance,
}

impl Default for Config {
    fn default() -> Self {
        Self::from_value(&Value::Object(Map::new()))
    }
}

impl Config {
    /// Keys with the wrong type fall back to their defaults.
    pub fn from_value(value: &Value) -> Self {
        let text = |key: &str| value.get(key).and_then(Value::as_str);
        let guidance = value.get("guidance");
        let guidance_text = |key: &str| {
            guidance
                .and_then(|g| g.get(key))
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
        };
        let guidelines = guidance
            .and_then(|g| g.get("promptGuidelines"))
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().map(str::to_owned))
                    .collect::<Option<Vec<String>>>()
            })
            .filter(|items| !items.is_empty() && items.iter().all(|s| !s.trim().is_empty()));
        Self {
            collapse_key: resolve_collapse_key(text("collapseKey")),
            notes_key: resolve_notes_key(text("notesKey")),
            guidance: Guidance {
                description: guidance_text("description"),
                prompt_snippet: guidance_text("promptSnippet"),
                prompt_guidelines: guidelines,
            },
        }
    }

    /// Upstream XDG file, then `<agentDir>/ext/ask-user-question.json`, then the project file.
    pub fn load(agent_dir: &Path, cwd: &Path) -> Self {
        let mut merged = Map::new();
        let files = [
            upstream_config_path(),
            Some(agent_dir.join("ext").join("ask-user-question.json")),
            Some(cwd.join(".pi").join("ext").join("ask-user-question.json")),
        ];
        for path in files.into_iter().flatten() {
            if let Some(Value::Object(map)) = read_json(&path) {
                merged.extend(map);
            }
        }
        Self::from_value(&Value::Object(merged))
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// `$XDG_CONFIG_HOME/rpiv-ask-user-question/config.json` when that exists, else the legacy path.
fn upstream_config_path() -> Option<PathBuf> {
    let home = home();
    let legacy = home
        .as_ref()
        .map(|h| h.join(".config/rpiv-ask-user-question/config.json"));
    let xdg = std::env::var("XDG_CONFIG_HOME").ok().and_then(|raw| {
        let expanded = match (raw.strip_prefix('~'), &home) {
            (Some(rest), Some(h)) => h.join(rest.trim_start_matches('/')),
            _ => PathBuf::from(&raw),
        };
        expanded
            .is_absolute()
            .then(|| expanded.join("rpiv-ask-user-question/config.json"))
    });
    match xdg {
        Some(path) if path.is_file() => Some(path),
        _ => legacy,
    }
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(value) => Some(value),
        Err(err) => {
            pi_ext_sdk::warn!("invalid JSON at {}, using defaults: {err}", path.display());
            None
        }
    }
}

pub fn is_valid_key_spec(spec: &str) -> bool {
    if spec.is_empty() || spec.starts_with('+') || spec.ends_with('+') || spec.contains("++") {
        return false;
    }
    let parts: Vec<&str> = spec.split('+').collect();
    let (base, modifiers) = parts.split_last().expect("non-empty split");
    let mut seen = Vec::new();
    for m in modifiers {
        if !MODIFIERS.contains(m) || seen.contains(m) {
            return false;
        }
        seen.push(m);
    }
    let mut chars = base.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => PRINTABLE.contains(c),
        _ => SPECIAL_KEYS.contains(base),
    }
}

pub fn resolve_collapse_key(raw: Option<&str>) -> String {
    let raw = raw.map(|r| r.trim().to_lowercase()).unwrap_or_default();
    if raw.is_empty() {
        return DEFAULT_COLLAPSE_KEY.into();
    }
    if raw == COLLAPSE_KEY_OFF || is_valid_key_spec(&raw) {
        return raw;
    }
    DEFAULT_COLLAPSE_KEY.into()
}

pub fn resolve_notes_key(raw: Option<&str>) -> String {
    let raw = raw.map(str::trim).unwrap_or_default();
    if raw.is_empty() {
        return DEFAULT_NOTES_KEY.into();
    }
    if raw.chars().count() == 1 {
        return raw.into();
    }
    let spec = raw.to_lowercase();
    if is_valid_key_spec(&spec) && !NOTES_KEY_RESERVED.contains(&spec.as_str()) {
        spec
    } else {
        DEFAULT_NOTES_KEY.into()
    }
}

/// `ctrl+]` shows as `Ctrl+]`; a single-character key shows as configured.
pub fn display_key(spec: &str) -> String {
    if spec.chars().count() == 1 {
        return spec.to_owned();
    }
    spec.split('+')
        .map(|part| match part {
            "pageup" => "PageUp".to_owned(),
            "pagedown" => "PageDown".to_owned(),
            _ => {
                let mut chars = part.chars();
                chars.next().map_or_else(String::new, |c| {
                    c.to_uppercase().collect::<String>() + chars.as_str()
                })
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Whether `key` is the key named by `spec`.
pub fn matches_spec(key: &Key, spec: &str) -> bool {
    if spec == COLLAPSE_KEY_OFF || spec.is_empty() {
        return false;
    }
    let mut spec_chars = spec.chars();
    if let (Some(c), None) = (spec_chars.next(), spec_chars.next()) {
        return *key == Key::Char(c);
    }
    let parts: Vec<&str> = spec.split('+').collect();
    let (base, modifiers) = parts.split_last().expect("non-empty split");
    let mut chars = base.chars();
    let single = match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    };
    if let (Some(c), ["ctrl"]) = (single, modifiers) {
        let upper = c.to_ascii_uppercase();
        if let Key::Unknown(raw) = key
            && ('@'..='_').contains(&upper)
            && raw.as_bytes() == [(upper as u8) & 0x1f]
        {
            return true;
        }
        return *key == Key::Ctrl(c.to_ascii_lowercase());
    }
    key.is(spec)
}
