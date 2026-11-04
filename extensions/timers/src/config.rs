use std::path::Path;

use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub max_sleep_seconds: u64,
    pub min_heartbeat_seconds: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_sleep_seconds: 3_600,
            min_heartbeat_seconds: 30,
        }
    }
}

fn read_object(path: &Path) -> Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

/// `<agentDir>/ext/timers.json`, then `<cwd>/.pi/ext/timers.json`; project keys win.
pub fn load(agent_dir: &Path, cwd: &Path) -> Config {
    let mut merged = read_object(&agent_dir.join("ext/timers.json"));
    merged.extend(read_object(&cwd.join(".pi/ext/timers.json")));
    merged.retain(|_, value| !value.is_null());
    serde_json::from_value(Value::Object(merged)).unwrap_or_else(|err| {
        pi_ext_sdk::warn!("timers config ignored: {err}");
        Config::default()
    })
}
