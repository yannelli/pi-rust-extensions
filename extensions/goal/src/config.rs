use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub disable_tasks: bool,
    pub disable_contracts: bool,
    pub disable_auditor: bool,
    pub max_autonomous_runs: Option<u32>,
    pub show_autonomous_runs: bool,
    pub strict_execution_contract: bool,
    pub hide_unfocused_prompt: bool,
    pub hide_unfocused_banner: bool,
    pub objective_max_chars: Option<usize>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub thinking_level: Option<String>,
    pub goals_root: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            disable_tasks: false,
            disable_contracts: false,
            disable_auditor: false,
            max_autonomous_runs: None,
            show_autonomous_runs: true,
            strict_execution_contract: false,
            hide_unfocused_prompt: false,
            hide_unfocused_banner: false,
            objective_max_chars: None,
            provider: None,
            model: None,
            thinking_level: None,
            goals_root: None,
        }
    }
}

impl Settings {
    /// `provider/model` for the auditor, when configured.
    pub fn auditor_model(&self) -> Option<String> {
        match (&self.provider, &self.model) {
            (_, Some(model)) if model.contains('/') => Some(model.clone()),
            (Some(provider), Some(model)) => Some(format!("{provider}/{model}")),
            _ => None,
        }
    }

    /// `goalsRoot` relative to `cwd` (`~/` expands), else `<agentDir>/ext/goal/projects/<cwd slug>`.
    pub fn goals_dir(&self, agent_dir: &Path, cwd: &Path) -> PathBuf {
        let Some(root) = self
            .goals_root
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
        else {
            return agent_dir.join("ext/goal/projects").join(project_slug(cwd));
        };
        if let Some(rest) = root.strip_prefix("~/") {
            if let Some(home) = std::env::var_os("HOME") {
                return PathBuf::from(home).join(rest);
            }
        }
        cwd.join(root)
    }
}

pub fn project_slug(cwd: &Path) -> String {
    let text = cwd.display().to_string();
    let slug: String = text
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "root".into()
    } else {
        slug.to_owned()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Project,
}

pub fn config_path(scope: Scope, agent_dir: &Path, cwd: &Path) -> PathBuf {
    match scope {
        Scope::Global => agent_dir.join("ext/goal.json"),
        Scope::Project => cwd.join(".pi/ext/goal.json"),
    }
}

fn layer_paths(agent_dir: &Path, cwd: &Path) -> [(PathBuf, bool); 4] {
    [
        (agent_dir.join("pi-goal-x-settings.json"), false),
        (config_path(Scope::Global, agent_dir, cwd), false),
        (cwd.join(".pi/pi-goal-x-settings.json"), true),
        (config_path(Scope::Project, agent_dir, cwd), true),
    ]
}

pub fn read_object(path: &Path) -> Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

/// Keys that pick the auditor model; project files set them only when the project is trusted.
pub const TRUSTED_KEYS: [&str; 3] = ["provider", "model", "thinkingLevel"];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Loaded {
    pub settings: Settings,
    pub ignored: Vec<String>,
}

/// Merges layers per key: upstream global, ext global, the `goal` settings namespace,
/// upstream project, ext project.
pub fn load(agent_dir: &Path, cwd: &Path, namespace: Option<&Value>, trusted: bool) -> Loaded {
    let mut merged = Map::new();
    let mut ignored = Vec::new();
    for (index, (path, project)) in layer_paths(agent_dir, cwd).into_iter().enumerate() {
        if index == 2 {
            if let Some(Value::Object(ns)) = namespace {
                let mut ns = ns.clone();
                if !trusted {
                    let global_settings = read_object(&agent_dir.join("settings.json"));
                    let global = global_settings.get("goal").and_then(Value::as_object);
                    for key in TRUSTED_KEYS {
                        let global_value = global.and_then(|global| global.get(key));
                        if ns
                            .remove(key)
                            .as_ref()
                            .is_some_and(|value| Some(value) != global_value)
                        {
                            ignored.push(key.to_owned());
                        }
                        if let Some(value) = global_value {
                            ns.insert(key.to_owned(), value.clone());
                        }
                    }
                }
                merged.extend(ns);
            }
        }
        let mut layer = read_object(&path);
        if project && !trusted {
            for key in TRUSTED_KEYS {
                if layer.remove(key).is_some() && !ignored.iter().any(|k| k == key) {
                    ignored.push(key.to_owned());
                }
            }
        }
        merged.extend(layer);
    }
    merged.retain(|_, value| !value.is_null());
    let settings = serde_json::from_value(Value::Object(merged)).unwrap_or_else(|err| {
        pi_ext_sdk::warn!("goal settings ignored: {err}");
        Settings::default()
    });
    Loaded { settings, ignored }
}

/// Sets (`Some`) or removes (`None`) one key in a scope file with a temp-file rename.
pub fn write_key(path: &Path, key: &str, value: Option<Value>) -> std::io::Result<()> {
    let mut object = read_object(path);
    match value {
        Some(value) => object.insert(key.to_owned(), value),
        None => object.remove(key),
    };
    let text = serde_json::to_string_pretty(&Value::Object(object))?;
    crate::store::write_atomic(path, text.as_bytes())
}
