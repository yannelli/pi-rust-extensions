use std::path::Path;

use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::frontmatter::substitute;
use crate::jsonfile;
use crate::stage::Vars;

/// Variables a stdio server process gets, as in Claude Code.
const SERVER_ENV: [&str; 3] = [
    "CLAUDE_PLUGIN_ROOT",
    "CLAUDE_PLUGIN_DATA",
    "CLAUDE_PROJECT_DIR",
];

/// Adds [`SERVER_ENV`] to the `env` of a stdio server; values the plugin sets stay.
fn add_env(config: &mut Map<String, Value>, vars: &Vars) {
    if !config.contains_key("command") {
        return;
    }
    let env = config.entry("env").or_insert_with(|| json!({}));
    let Value::Object(env) = env else { return };
    for (key, value) in vars {
        if SERVER_ENV.contains(&key.as_str()) && !env.contains_key(key) {
            env.insert(key.clone(), json!(value));
        }
    }
}

/// Field that marks an `mcpServers` entry as written for a plugin.
pub const TAG: &str = "claudePlugin";

fn substitute_all(value: &mut Value, vars: &Vars) {
    match value {
        Value::String(text) => *text = substitute(text, vars),
        Value::Array(items) => items.iter_mut().for_each(|v| substitute_all(v, vars)),
        Value::Object(map) => map.values_mut().for_each(|v| substitute_all(v, vars)),
        _ => {}
    }
}

pub fn server_key(plugin: &str, server: &str) -> String {
    format!("plugin_{plugin}_{server}")
}

fn owned_by(entry: &Value, plugin_id: &str) -> bool {
    entry.get(TAG).and_then(Value::as_str) == Some(plugin_id)
}

/// Replaces the servers of `plugin_id` in `file`. Returns the written keys and notes for
/// keys that belong to someone else.
pub fn write(
    file: &Path,
    plugin_id: &str,
    plugin: &str,
    servers: &Map<String, Value>,
    vars: &Vars,
) -> Result<(Vec<String>, Vec<String>)> {
    let mut data = jsonfile::read_object(file)?;
    let section = jsonfile::object_entry(&mut data, "mcpServers")?;
    let before = section.len();
    section.retain(|_, entry| !owned_by(entry, plugin_id));
    let removed = section.len() != before;
    let mut keys = Vec::new();
    let mut notes = Vec::new();
    for (name, config) in servers {
        let key = server_key(plugin, name);
        if section.contains_key(&key) {
            notes.push(format!(
                "MCP server {key} already exists in {}; kept it",
                file.display()
            ));
            continue;
        }
        let Value::Object(config) = config else {
            notes.push(format!("MCP server {name} is not an object; skipped"));
            continue;
        };
        let mut config = Value::Object(config.clone());
        substitute_all(&mut config, vars);
        if let Value::Object(map) = &mut config {
            add_env(map, vars);
        }
        config[TAG] = json!(plugin_id);
        section.insert(key.clone(), config);
        keys.push(key);
    }
    if removed || !keys.is_empty() {
        jsonfile::write(file, &Value::Object(data))?;
    }
    Ok((keys, notes))
}

pub fn remove(file: &Path, plugin_id: &str) -> Result<()> {
    write(file, plugin_id, "", &Map::new(), &Vars::new()).map(|_| ())
}
