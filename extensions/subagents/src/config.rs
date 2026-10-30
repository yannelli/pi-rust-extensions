//! `<agentDir>/ext/subagents.json` and `<cwd>/.pi/ext/subagents.json`; project keys win.

use std::path::Path;

use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub async_by_default: bool,
    pub max_parallel: usize,
    pub max_tasks: usize,
    pub max_recent_runs: usize,
    pub widget: bool,
    pub advertise: bool,
    pub pi_bin: Option<String>,
    pub extra_args: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            async_by_default: false,
            max_parallel: 4,
            max_tasks: 8,
            max_recent_runs: 20,
            widget: true,
            advertise: true,
            pi_bin: None,
            extra_args: Vec::new(),
        }
    }
}

/// Keys that start processes; the project file sets them only in a trusted project.
pub const TRUSTED_KEYS: [&str; 2] = ["piBin", "extraArgs"];

pub struct Loaded {
    pub config: Config,
    /// Project keys skipped because the project is not trusted.
    pub ignored: Vec<String>,
}

pub fn load(agent_dir: &Path, project_dir: &Path, trusted: bool) -> Loaded {
    let mut merged = read(&agent_dir.join("ext").join("subagents.json"));
    let mut ignored = Vec::new();
    for (key, value) in read(&project_dir.join("ext").join("subagents.json")) {
        if !trusted && TRUSTED_KEYS.contains(&key.as_str()) {
            ignored.push(key);
            continue;
        }
        merged.insert(key, value);
    }
    Loaded {
        config: from_map(&merged),
        ignored,
    }
}

fn read(path: &Path) -> Map<String, Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Map::new();
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => map,
        _ => {
            pi_ext_sdk::warn!("ignoring {}: not a JSON object", path.display());
            Map::new()
        }
    }
}

pub fn from_map(map: &Map<String, Value>) -> Config {
    let d = Config::default();
    let bool_of =
        |key: &str, default: bool| map.get(key).and_then(Value::as_bool).unwrap_or(default);
    let num_of = |key: &str, default: usize| {
        map.get(key)
            .and_then(Value::as_u64)
            .map_or(default, |n| (n as usize).max(1))
    };
    Config {
        async_by_default: bool_of("asyncByDefault", d.async_by_default),
        max_parallel: num_of("maxParallel", d.max_parallel),
        max_tasks: num_of("maxTasks", d.max_tasks),
        max_recent_runs: num_of("maxRecentRuns", d.max_recent_runs),
        widget: bool_of("widget", d.widget),
        advertise: bool_of("advertise", d.advertise),
        pi_bin: map.get("piBin").and_then(Value::as_str).map(str::to_owned),
        extra_args: map
            .get("extraArgs")
            .and_then(Value::as_array)
            .map(|args| {
                args.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    }
}
