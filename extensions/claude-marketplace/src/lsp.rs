//! Stages the LSP servers of a plugin in the lens server file (`<root>/ext/lens-lsp.json`).

use std::path::Path;

use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::fields::Dropped;
use crate::frontmatter::substitute;
use crate::jsonfile;
use crate::stage::Vars;

/// Field that names the owner of a lens server entry.
pub const TAG: &str = "plugin";

/// Server fields that lens applies. Claude Code substitutes variables in the first four.
const APPLIED: [&str; 12] = [
    "command",
    "args",
    "env",
    "workspaceFolder",
    "extensionToLanguage",
    "transport",
    "initializationOptions",
    "settings",
    "startupTimeout",
    "shutdownTimeout",
    "requestTimeout",
    "diagnostics",
];

/// Server fields that lens reads but cannot apply.
const NOT_APPLIED: [&str; 2] = ["restartOnCrash", "maxRestarts"];
const RESTART_REASON: &str =
    "lens starts a new server for each request, so there is no server to restart";

/// What `write` staged.
#[derive(Debug, Default)]
pub struct Staged {
    /// Keys in the server file.
    pub keys: Vec<String>,
    /// `<key>: <command> (<extensions>)` of each staged server, for `info`.
    pub lines: Vec<String>,
    pub notes: Vec<String>,
    pub dropped: Vec<Dropped>,
}

pub fn server_key(plugin: &str, server: &str) -> String {
    format!("plugin_{plugin}_{server}")
}

fn owned_by(entry: &Value, plugin_id: &str) -> bool {
    entry.get(TAG).and_then(Value::as_str) == Some(plugin_id)
}

fn substitute_value(value: &mut Value, vars: &Vars) {
    match value {
        Value::String(text) => *text = substitute(text, vars),
        Value::Array(items) => items.iter_mut().for_each(|v| substitute_value(v, vars)),
        Value::Object(map) => map.values_mut().for_each(|v| substitute_value(v, vars)),
        _ => {}
    }
}

/// Checks one Claude server config and turns it into a lens entry. `Err` is the reason
/// the server is skipped.
fn convert(
    name: &str,
    config: &Value,
    vars: &Vars,
    env: &Vars,
    dropped: &mut Vec<Dropped>,
) -> Result<Map<String, Value>, String> {
    let Value::Object(config) = config else {
        return Err("it is not an object".into());
    };
    let mut entry = Map::new();
    for (key, value) in config {
        let what = format!("LSP server {name} {key}");
        if NOT_APPLIED.contains(&key.as_str()) {
            dropped.push(Dropped::new(what, RESTART_REASON));
        } else if APPLIED.contains(&key.as_str()) {
            entry.insert(key.clone(), value.clone());
        } else {
            dropped.push(Dropped::new(what, "unknown field; Claude Code rejects it"));
        }
    }
    let command = entry.get("command").and_then(Value::as_str);
    if command.is_none_or(str::is_empty) {
        return Err("it needs command".into());
    }
    let map = entry.get("extensionToLanguage").and_then(Value::as_object);
    let Some(map) = map.filter(|m| !m.is_empty()) else {
        return Err("it needs extensionToLanguage with at least one extension".into());
    };
    if let Some((ext, _)) = map
        .iter()
        .find(|(k, v)| !k.starts_with('.') || !v.is_string())
    {
        return Err(format!(
            "extensionToLanguage \"{ext}\" needs a leading dot and a language ID"
        ));
    }
    check_types(&entry)?;
    for key in ["command", "args", "env", "workspaceFolder"] {
        if let Some(value) = entry.get_mut(key) {
            substitute_value(value, vars);
        }
    }
    if let Some(Value::String(dir)) = entry.get_mut("workspaceFolder") {
        *dir = dir.replace("${CLAUDE_PROJECT_DIR}", ".");
    }
    let target = entry.entry("env").or_insert_with(|| json!({}));
    let Value::Object(target) = target else {
        return Err("env is not an object".into());
    };
    for (key, value) in env {
        if !target.contains_key(key) {
            target.insert(key.clone(), json!(value));
        }
    }
    Ok(entry)
}

/// The value types that Claude Code accepts, so lens does not skip the entry.
fn check_types(entry: &Map<String, Value>) -> Result<(), String> {
    let strings = |value: Option<&Value>| match value {
        None => true,
        Some(Value::Array(items)) => items.iter().all(Value::is_string),
        Some(Value::Object(map)) => map.values().all(Value::is_string),
        Some(_) => false,
    };
    if !entry.get("args").is_none_or(Value::is_array) || !strings(entry.get("args")) {
        return Err("args must be a list of strings".into());
    }
    if !entry.get("env").is_none_or(Value::is_object) || !strings(entry.get("env")) {
        return Err("env must be an object of strings".into());
    }
    for key in ["startupTimeout", "shutdownTimeout", "requestTimeout"] {
        if entry
            .get(key)
            .is_some_and(|v| v.as_u64().is_none_or(|n| n == 0))
        {
            return Err(format!("{key} must be a positive integer"));
        }
    }
    for (key, ok) in [
        (
            "workspaceFolder",
            entry.get("workspaceFolder").is_none_or(Value::is_string),
        ),
        (
            "transport",
            entry.get("transport").is_none_or(Value::is_string),
        ),
        (
            "diagnostics",
            entry.get("diagnostics").is_none_or(Value::is_boolean),
        ),
    ] {
        if !ok {
            return Err(format!("{key} has the wrong type"));
        }
    }
    Ok(())
}

fn describe(key: &str, entry: &Map<String, Value>) -> String {
    let command = entry["command"].as_str().unwrap_or_default();
    let exts: Vec<&str> = entry["extensionToLanguage"]
        .as_object()
        .map(|m| m.keys().map(String::as_str).collect())
        .unwrap_or_default();
    format!("{key}: {command} ({})", exts.join(", "))
}

/// Replaces the servers of `plugin_id` in `file`. `vars` are the substitutions and `env`
/// the plugin env. Keys that belong to someone else are kept, with a note.
pub fn write(
    file: &Path,
    plugin_id: &str,
    plugin: &str,
    servers: &Map<String, Value>,
    vars: &Vars,
    env: &Vars,
) -> Result<Staged> {
    let mut data = jsonfile::read_object(file)?;
    let section = jsonfile::object_entry(&mut data, "servers")?;
    let before = section.len();
    section.retain(|_, entry| !owned_by(entry, plugin_id));
    let removed = section.len() != before;
    let mut staged = Staged::default();
    for (name, config) in servers {
        let key = server_key(plugin, name);
        if section.contains_key(&key) {
            let note = format!(
                "LSP server {key} already exists in {}; kept it",
                file.display()
            );
            staged.notes.push(note);
            continue;
        }
        let mut entry = match convert(name, config, vars, env, &mut staged.dropped) {
            Ok(entry) => entry,
            Err(reason) => {
                let what = format!("LSP server {name}");
                staged.dropped.push(Dropped::new(what, reason));
                continue;
            }
        };
        entry.insert(TAG.into(), json!(plugin_id));
        staged.lines.push(describe(&key, &entry));
        section.insert(key.clone(), Value::Object(entry));
        staged.keys.push(key);
    }
    if removed || !staged.keys.is_empty() {
        jsonfile::write(file, &Value::Object(data))?;
    }
    Ok(staged)
}

pub fn remove(file: &Path, plugin_id: &str) -> Result<()> {
    write(file, plugin_id, "", &Map::new(), &Vars::new(), &Vars::new()).map(|_| ())
}
