use std::path::Path;

use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::frontmatter::substitute;
use crate::jsonfile;

/// Field that marks an `mcpServers` entry as written for a plugin.
pub const TAG: &str = "claudePlugin";

fn substitute_all(value: &mut Value, vars: &[(&str, String)]) {
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
    vars: &[(&str, String)],
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
        let Value::Object(_) = config else {
            notes.push(format!("MCP server {name} is not an object; skipped"));
            continue;
        };
        let mut config = config.clone();
        substitute_all(&mut config, vars);
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
    write(file, plugin_id, "", &Map::new(), &[]).map(|_| ())
}
