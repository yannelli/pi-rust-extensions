//! Which iTerm2 features are on. The value is `true`, `false`, or an object of features in
//! which a feature that is not named is on, as in the old `terminal.iterm2` setting.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Features {
    pub tab_color: bool,
    pub badge: bool,
    pub user_vars: bool,
    pub attention: bool,
    pub cwd: bool,
}

impl Features {
    pub const ALL: Features = Features {
        tab_color: true,
        badge: true,
        user_vars: true,
        attention: true,
        cwd: true,
    };

    pub fn from_value(value: Option<&Value>) -> Self {
        let on = |key: &str| match value {
            Some(Value::Bool(enabled)) => *enabled,
            Some(Value::Object(object)) => {
                object.get("enabled").and_then(Value::as_bool) != Some(false)
                    && object.get(key).and_then(Value::as_bool).unwrap_or(true)
            }
            _ => false,
        };
        Self {
            tab_color: on("tabColor"),
            badge: on("badge"),
            user_vars: on("userVars"),
            attention: on("attention"),
            cwd: on("cwd"),
        }
    }

    pub fn any(&self) -> bool {
        self.tab_color || self.badge || self.user_vars || self.attention || self.cwd
    }
}

pub fn global_config_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join("iterm2.json")
}

pub fn project_config_path(cwd: &Path) -> PathBuf {
    cwd.join(".pi").join("ext").join("iterm2.json")
}

/// A config file as an object; `true` and `false` become `{ "enabled": .. }`.
pub fn read_config(path: &Path) -> Option<Map<String, Value>> {
    let value: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    match value {
        Value::Object(object) => Some(object),
        Value::Bool(enabled) => Some(Map::from_iter([("enabled".into(), json!(enabled))])),
        _ => None,
    }
}

/// The effective value: project keys over global keys, else the old `terminal.iterm2`
/// setting, else all features on.
pub fn resolve(
    global: Option<Map<String, Value>>,
    project: Option<Map<String, Value>>,
    legacy: Option<Value>,
) -> Value {
    match (global, project) {
        (None, None) => legacy.filter(|v| !v.is_null()).unwrap_or(json!(true)),
        (global, project) => {
            let mut merged = global.unwrap_or_default();
            merged.extend(project.unwrap_or_default());
            Value::Object(merged)
        }
    }
}

/// Turns the integration on in the global config. `false` when it was already on.
pub fn enable(path: &Path, effective: &Value) -> std::io::Result<bool> {
    if Features::from_value(Some(effective)).any() {
        return Ok(false);
    }
    let mut config = read_config(path).unwrap_or_default();
    config.insert("enabled".into(), json!(true));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(config))?;
    std::fs::write(path, text + "\n")?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn features(value: Value) -> Features {
        Features::from_value(Some(&value))
    }

    #[test]
    fn iterm2_features_are_off_unless_turned_on() {
        assert!(!Features::from_value(None).any());
        assert!(!features(json!(false)).any());
        assert_eq!(features(json!(true)), Features::ALL);
        let some = features(json!({"badge": false, "cwd": false}));
        assert!(some.tab_color && !some.badge && some.user_vars && some.attention && !some.cwd);
        assert!(!features(json!("yes")).any());
        assert!(!features(json!({"enabled": false, "badge": true})).any());
    }

    #[test]
    fn project_config_wins_and_the_old_setting_is_the_fallback() {
        let object = |value: Value| value.as_object().cloned();
        assert_eq!(resolve(None, None, None), json!(true));
        assert_eq!(resolve(None, None, Some(json!(false))), json!(false));
        assert_eq!(
            resolve(None, None, Some(json!({"badge": false}))),
            json!({"badge": false})
        );
        let merged = resolve(
            object(json!({"badge": false, "cwd": false})),
            object(json!({"cwd": true})),
            Some(json!(false)),
        );
        assert_eq!(merged, json!({"badge": false, "cwd": true}));
    }

    #[test]
    fn enable_writes_the_global_config_only_when_off() {
        let dir = std::env::temp_dir().join(format!("pi-iterm2-{}", std::process::id()));
        let path = global_config_path(&dir);
        assert!(!enable(&path, &json!(true)).unwrap());
        assert!(!path.exists());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"enabled": false, "badge": false}"#).unwrap();
        assert!(enable(&path, &json!({"enabled": false})).unwrap());
        let written = read_config(&path).unwrap();
        assert_eq!(
            Value::Object(written),
            json!({"enabled": true, "badge": false})
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
