use std::path::Path;

use serde_json::{Map, Value};

pub const DEFAULT_MAX_DEPTH: f64 = 10.0;
pub const DEFAULT_MAX_BYTES: f64 = 500_000.0;
pub const FILE_NAME: &str = "context-include.json";

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub max_depth: f64,
    pub max_bytes: f64,
    /// `None` means the default allow roots.
    pub allowed_roots: Option<Vec<String>>,
    pub denied_roots: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            max_bytes: DEFAULT_MAX_BYTES,
            allowed_roots: None,
            denied_roots: Vec::new(),
        }
    }
}

impl Config {
    /// Parses a `contextInclude` block; non-objects give the defaults.
    pub fn from_value(value: Option<&Value>) -> Self {
        let Some(obj) = value.and_then(Value::as_object) else {
            return Self::default();
        };
        Self {
            max_depth: valid_limit(obj.get("maxDepth")).unwrap_or(DEFAULT_MAX_DEPTH),
            max_bytes: valid_limit(obj.get("maxBytes")).unwrap_or(DEFAULT_MAX_BYTES),
            allowed_roots: string_array(obj.get("allowedRoots")),
            denied_roots: string_array(obj.get("deniedRoots")).unwrap_or_default(),
        }
    }
}

fn valid_limit(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n >= 0.0)
}

fn string_array(value: Option<&Value>) -> Option<Vec<String>> {
    value?
        .as_array()?
        .iter()
        .map(|v| v.as_str().map(str::to_owned))
        .collect()
}

/// File contents as JSON; `None` when the file cannot be read, `{}` when it does not parse.
pub fn read_file(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(serde_json::from_str(&text).unwrap_or_else(|_| Value::Object(Map::new())))
}

/// Order: project ext file, then the `contextInclude` setting, then the global ext file.
/// `deniedRoots` of the setting and the global file apply whichever source wins.
pub fn select(
    project_file: Option<Value>,
    setting: Option<Value>,
    global_file: Option<Value>,
) -> Config {
    let setting = setting.filter(Value::is_object);
    let always_denied: Vec<String> = [&setting, &global_file]
        .into_iter()
        .flat_map(|source| Config::from_value(source.as_ref()).denied_roots)
        .collect();
    let chosen = project_file.or(setting).or(global_file);
    let mut config = Config::from_value(chosen.as_ref());
    for root in always_denied {
        if !config.denied_roots.contains(&root) {
            config.denied_roots.push(root);
        }
    }
    config
}

pub fn project_path(cwd: &Path) -> std::path::PathBuf {
    cwd.join(".pi").join("ext").join(FILE_NAME)
}

pub fn global_path(agent_dir: &Path) -> std::path::PathBuf {
    agent_dir.join("ext").join(FILE_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_fields_and_rejects_invalid() {
        let c = Config::from_value(Some(&json!({
            "maxDepth": 3, "maxBytes": 1024.5, "allowedRoots": ["/a", "b"], "deniedRoots": ["/d"]
        })));
        assert_eq!(c.max_depth, 3.0);
        assert_eq!(c.max_bytes, 1024.5);
        assert_eq!(c.allowed_roots, Some(vec!["/a".into(), "b".into()]));
        assert_eq!(c.denied_roots, vec!["/d".to_owned()]);

        let bad = Config::from_value(Some(&json!({
            "maxDepth": -1, "maxBytes": "10", "allowedRoots": ["/a", 1], "deniedRoots": "x"
        })));
        assert_eq!(bad, Config::default());
        assert_eq!(Config::from_value(Some(&json!([1]))), Config::default());
        assert_eq!(
            Config::from_value(Some(&json!({ "allowedRoots": [] }))).allowed_roots,
            Some(vec![])
        );
    }

    #[test]
    fn precedence_project_then_setting_then_global() {
        let p = Some(json!({ "maxDepth": 1 }));
        let s = Some(json!({ "maxDepth": 2 }));
        let g = Some(json!({ "maxDepth": 3 }));
        assert_eq!(select(p.clone(), s.clone(), g.clone()).max_depth, 1.0);
        assert_eq!(select(None, s.clone(), g.clone()).max_depth, 2.0);
        assert_eq!(select(None, None, g.clone()).max_depth, 3.0);
        assert_eq!(select(None, Some(json!(5)), g).max_depth, 3.0);
        assert_eq!(
            select(Some(json!({})), s, None).max_depth,
            DEFAULT_MAX_DEPTH
        );
        assert_eq!(select(None, None, None), Config::default());
    }

    #[test]
    fn global_denied_roots_survive_a_project_file() {
        let p = Some(json!({ "deniedRoots": ["/p"], "allowedRoots": ["/"] }));
        let s = Some(json!({ "deniedRoots": ["/s"] }));
        let g = Some(json!({ "deniedRoots": ["/g", "/p"] }));
        let config = select(p, s, g);
        assert_eq!(config.denied_roots, ["/p", "/s", "/g"]);
        assert_eq!(config.allowed_roots, Some(vec!["/".to_owned()]));
    }

    #[test]
    fn unparsable_file_reads_as_empty_object() {
        let dir = tempfile::tempdir().unwrap();
        let path = project_path(dir.path());
        assert_eq!(read_file(&path), None);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ nope").unwrap();
        assert_eq!(read_file(&path), Some(json!({})));
        std::fs::write(&path, r#"{"maxBytes": 7}"#).unwrap();
        assert_eq!(read_file(&path), Some(json!({ "maxBytes": 7 })));
    }
}
