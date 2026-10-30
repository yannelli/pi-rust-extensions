//! Where the status line config comes from: `<agentDir>/ext/statusline.json` and
//! `<cwd>/.pi/ext/statusline.json`, else the old `statusLine` setting of pi.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

pub type Layer = Option<Map<String, Value>>;

/// Types that run a shell command.
const RUNNERS: &[&str] = &["command", "claude", "cursor"];

pub fn global_config_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join("statusline.json")
}

pub fn project_config_path(cwd: &Path) -> PathBuf {
    cwd.join(".pi").join("ext").join("statusline.json")
}

pub fn read_object(path: &Path) -> Option<Map<String, Value>> {
    match serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()? {
        Value::Object(object) => Some(object),
        _ => None,
    }
}

/// `statusLine` of pi's global `settings.json`.
pub fn legacy_global(agent_dir: &Path) -> Option<Map<String, Value>> {
    match read_object(&agent_dir.join("settings.json"))?.remove("statusLine")? {
        Value::Object(object) => Some(object),
        _ => None,
    }
}

/// The global and the project layer: the extension files when one exists, else pi's global
/// `statusLine` and the merged `settings.get("statusLine")`.
pub fn layers(
    global_ext: Option<Map<String, Value>>,
    project_ext: Option<Map<String, Value>>,
    legacy_global: Option<Map<String, Value>>,
    legacy_merged: Option<Value>,
) -> (Layer, Layer) {
    if global_ext.is_some() || project_ext.is_some() {
        return (global_ext, project_ext);
    }
    let merged = match legacy_merged {
        Some(Value::Object(object)) => Some(object),
        _ => None,
    };
    (legacy_global, merged)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Effective {
    pub value: Value,
    /// A project layer tried to run a command in an untrusted project.
    pub ignored_command: bool,
}

/// Project keys over global keys. Without trust, `command` and command types come from the
/// global layer only, so a project cannot make pi run a command.
pub fn effective(
    global: Option<Map<String, Value>>,
    project: Option<Map<String, Value>>,
    trusted: bool,
) -> Effective {
    if global.is_none() && project.is_none() {
        return Effective {
            value: json!({"type": "builtin"}),
            ignored_command: false,
        };
    }
    let global = global.unwrap_or_default();
    let mut merged = global.clone();
    merged.extend(project.unwrap_or_default());
    let mut ignored_command = false;
    if !trusted {
        if merged.get("command") != global.get("command") {
            ignored_command = true;
            match global.get("command") {
                Some(command) => merged.insert("command".into(), command.clone()),
                None => merged.remove("command"),
            };
        }
        let kind = merged
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("builtin");
        if RUNNERS.contains(&kind) && merged.get("type") != global.get("type") {
            ignored_command = true;
            match global.get("type") {
                Some(kind) => merged.insert("type".into(), kind.clone()),
                None => merged.remove("type"),
            };
        }
    }
    Effective {
        value: Value::Object(merged),
        ignored_command,
    }
}

pub fn write_global(agent_dir: &Path, value: &Value) -> Result<(), String> {
    let path = global_config_path(agent_dir);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(&path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(value: Value) -> Option<Map<String, Value>> {
        value.as_object().cloned()
    }

    #[test]
    fn untrusted_projects_cannot_run_commands() {
        let global = object(json!({"type": "builtin", "preset": "pills", "command": "global.sh"}));
        let project = object(json!({"type": "command", "command": "evil.sh", "theme": "nord"}));
        let result = effective(global.clone(), project.clone(), false);
        assert!(result.ignored_command);
        assert_eq!(
            result.value,
            json!({"type": "builtin", "preset": "pills", "command": "global.sh", "theme": "nord"})
        );
        let trusted = effective(global, project, true);
        assert!(!trusted.ignored_command);
        assert_eq!(trusted.value["command"], "evil.sh");
        assert_eq!(trusted.value["type"], "command");

        let claude = effective(None, object(json!({"type": "claude"})), false);
        assert!(claude.ignored_command);
        assert!(claude.value.get("type").is_none());
        let global_command = object(json!({"type": "command", "command": "mine.sh"}));
        let kept = effective(global_command, object(json!({"padding": 2})), false);
        assert!(!kept.ignored_command);
        assert_eq!(
            kept.value,
            json!({"type": "command", "command": "mine.sh", "padding": 2})
        );
    }

    #[test]
    fn extension_files_win_over_the_old_setting() {
        let legacy = Some(json!({"type": "command", "command": "project.sh"}));
        let global = object(json!({"type": "command", "command": "global.sh"}));
        let (g, p) = layers(None, None, global.clone(), legacy.clone());
        let result = effective(g, p, false);
        assert_eq!(result.value["command"], "global.sh");
        let (g, p) = layers(object(json!({"preset": "pi"})), None, global, legacy);
        assert_eq!(effective(g, p, false).value, json!({"preset": "pi"}));
        assert_eq!(
            effective(None, None, false).value,
            json!({"type": "builtin"})
        );
    }
}
