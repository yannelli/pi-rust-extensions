use std::path::Path;
use std::time::Duration;

use pi_ext_sdk::Host;
use pi_ext_sdk::json::merge_patch;
use serde_json::{Value, json};

pub use crate::excludes::DEFAULT_EXCLUDE_PATTERNS;

pub const DEFAULT_STAGE_BUDGET: Duration = Duration::from_secs(10);
pub const DEFAULT_GIT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreOnTree {
    Ask,
    Never,
    Always,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub enabled: bool,
    pub auto_checkpoint: bool,
    pub restore_on_fork: bool,
    pub restore_on_clone: bool,
    pub restore_on_resume: bool,
    pub restore_on_tree: RestoreOnTree,
    /// Defaults, then user `exclude`, then `include` as `!pattern` lines.
    pub exclude: Vec<String>,
    pub max_file_bytes: Option<u64>,
    /// How long a turn waits for its pre-turn snapshot before it runs without one.
    pub stage_budget: Duration,
    /// How long git may take to store one batch of files.
    pub git_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self::from_value(&json!({}))
    }
}

impl Config {
    /// Reads the `ayu` tree shape: `{ "rewind": {...}, "checkpoint": {...} }`.
    pub fn from_value(ayu: &Value) -> Self {
        let checkpoint = &ayu["checkpoint"];
        let flag = |key: &str, default: bool| checkpoint[key].as_bool().unwrap_or(default);
        let strings = |key: &str| -> Vec<String> {
            checkpoint[key]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut exclude: Vec<String> = DEFAULT_EXCLUDE_PATTERNS
            .iter()
            .map(|pattern| pattern.to_string())
            .collect();
        exclude.extend(strings("exclude"));
        exclude.extend(strings("include").into_iter().map(|pattern| {
            if pattern.starts_with('!') {
                pattern
            } else {
                format!("!{pattern}")
            }
        }));
        let max_file_bytes = checkpoint["maxFileMB"]
            .as_f64()
            .filter(|mb| mb.is_finite() && *mb > 0.0)
            .map(|mb| (mb * 1024.0 * 1024.0) as u64);
        let stage_budget = checkpoint["stageBudgetSeconds"]
            .as_f64()
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
            .map_or(DEFAULT_STAGE_BUDGET, Duration::from_secs_f64);
        let git_timeout = checkpoint["gitTimeoutSeconds"]
            .as_f64()
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
            .map_or(DEFAULT_GIT_TIMEOUT, Duration::from_secs_f64);
        let restore_on_tree = match ayu["rewind"]["restoreOnTree"].as_str() {
            Some("never") => RestoreOnTree::Never,
            Some("always") => RestoreOnTree::Always,
            _ => RestoreOnTree::Ask,
        };
        Self {
            enabled: flag("enabled", true),
            auto_checkpoint: flag("autoCheckpoint", true),
            restore_on_fork: flag("restoreOnFork", false),
            restore_on_clone: flag("restoreOnClone", false),
            restore_on_resume: flag("restoreOnResume", false),
            restore_on_tree,
            exclude,
            max_file_bytes,
            stage_budget,
            git_timeout,
        }
    }

    pub fn checkpoints_on(&self) -> bool {
        self.enabled && self.auto_checkpoint
    }
}

/// Later layers win per key; arrays replace.
pub fn merge_layers(layers: &[Value]) -> Value {
    let mut merged = json!({});
    for layer in layers.iter().filter(|layer| layer.is_object()) {
        merge_patch(&mut merged, &strip_nulls(layer));
    }
    merged
}

fn strip_nulls(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), strip_nulls(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn read_json(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|err| format!("Ignored malformed {}: {err}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(err) => Err(format!("Cannot read {}: {err}", path.display())),
    }
}

/// Drops project keys that restore files on their own when the project is not trusted.
pub fn untrusted_project(mut layer: Value) -> (Value, bool) {
    let mut dropped = false;
    if let Some(checkpoint) = layer["checkpoint"].as_object_mut() {
        for key in ["restoreOnResume", "restoreOnFork", "restoreOnClone"] {
            dropped |= checkpoint.remove(key).is_some();
        }
    }
    if layer["rewind"]["restoreOnTree"] == "always"
        && let Some(rewind) = layer["rewind"].as_object_mut()
    {
        dropped |= rewind.remove("restoreOnTree").is_some();
    }
    (layer, dropped)
}

/// Only the restore-trigger keys of `layer`.
pub fn restore_keys(layer: &Value) -> Value {
    let mut kept = json!({});
    for key in ["restoreOnResume", "restoreOnFork", "restoreOnClone"] {
        if let Some(value) = layer["checkpoint"].get(key) {
            kept["checkpoint"][key] = value.clone();
        }
    }
    if let Some(value) = layer["rewind"].get("restoreOnTree") {
        kept["rewind"]["restoreOnTree"] = value.clone();
    }
    kept
}

/// Layers in merge order. Without trust, restore triggers come only from global sources.
pub fn trusted_layers(
    merged_settings: Value,
    global_settings: Value,
    global: Value,
    project: Value,
    project_trusted: bool,
) -> (Vec<Value>, bool) {
    if project_trusted {
        return (vec![merged_settings, global, project], false);
    }
    let (settings, settings_dropped) = untrusted_project(merged_settings);
    let (project, project_dropped) = untrusted_project(project);
    let layers = vec![settings, restore_keys(&global_settings), global, project];
    (layers, settings_dropped || project_dropped)
}

/// pi settings `ayu`, then `<agentDir>/ext/rewind.json`, then `<cwd>/.pi/ext/rewind.json`.
/// Returns the config and messages to show the user.
pub async fn load(host: &Host, project_trusted: bool) -> (Config, Vec<String>) {
    let mut notes = Vec::new();
    let settings = host
        .settings::<Value>("ayu")
        .await
        .ok()
        .flatten()
        .unwrap_or(Value::Null);
    let mut layer = |path: &Path| {
        read_json(path).unwrap_or_else(|note| {
            notes.push(note);
            Value::Null
        })
    };
    let global_settings = if project_trusted {
        Value::Null
    } else {
        layer(&host.agent_dir().join("settings.json"))["ayu"].clone()
    };
    let global = layer(&host.agent_dir().join("ext").join("rewind.json"));
    let project = layer(&host.project_dir().join("ext").join("rewind.json"));
    let (layers, dropped) =
        trusted_layers(settings, global_settings, global, project, project_trusted);
    if dropped {
        notes.push(
            "Ignored project restore settings (restoreOnResume, restoreOnFork, restoreOnClone, \
             restoreOnTree \"always\"): the project is not trusted."
                .to_string(),
        );
    }
    (Config::from_value(&merge_layers(&layers)), notes)
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
