//! Reads and edits `<agentDir>/ext/tool-results.json` and the project file.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::config::{Config, FILE_NAME};

pub fn global_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join(FILE_NAME)
}

pub fn project_path(cwd: &Path) -> PathBuf {
    cwd.join(".pi").join("ext").join(FILE_NAME)
}

/// File contents as JSON; `None` when missing, `{}` when it does not parse.
pub fn read_file(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(serde_json::from_str(&text).unwrap_or_else(|_| json!({})))
}

/// The global file, with a trusted project file merged over it (RFC 7386).
pub fn load(global: Option<Value>, project: Option<Value>) -> Config {
    let mut value = global.filter(Value::is_object).unwrap_or_else(|| json!({}));
    if let Some(project) = project.filter(Value::is_object) {
        pi_ext_sdk::json::merge_patch(&mut value, &project);
    }
    Config::from_value(&value)
}

fn object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = json!({});
    }
    value.as_object_mut().expect("object")
}

fn tools(root: &mut Value) -> &mut Map<String, Value> {
    object(object(root).entry("tools").or_insert_with(|| json!({})))
}

/// Sets (`Some`) or removes (`None`) `key` at the top level, or in `tools.<rule>` when given.
pub fn edit(
    path: &Path,
    rule: Option<&str>,
    key: &str,
    value: Option<Value>,
) -> std::io::Result<()> {
    let mut root = read_file(path).unwrap_or_else(|| json!({}));
    let target = match rule {
        None => object(&mut root),
        Some(pattern) => object(
            tools(&mut root)
                .entry(pattern.to_owned())
                .or_insert_with(|| json!({})),
        ),
    };
    match value {
        Some(value) => target.insert(key.to_owned(), value),
        None => target.remove(key),
    };
    write_file(path, &root)
}

/// Adds an empty rule for `pattern`, or removes it with `remove`.
pub fn edit_rule(path: &Path, pattern: &str, remove: bool) -> std::io::Result<()> {
    let mut root = read_file(path).unwrap_or_else(|| json!({}));
    let tools = tools(&mut root);
    if remove {
        tools.remove(pattern);
    } else {
        tools.entry(pattern.to_owned()).or_insert_with(|| json!({}));
    }
    write_file(path, &root)
}

pub fn write_file(path: &Path, value: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    std::fs::write(path, text + "\n")
}
