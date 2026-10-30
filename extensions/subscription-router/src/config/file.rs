//! Reading and writing the config file.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::{CONFIG_ENV, Config, Policy, Profile, parse_config};

pub fn profile_json(profile: &Profile) -> Value {
    let chain: Vec<Value> = profile
        .chain
        .iter()
        .map(|entry| match &entry.thinking {
            Some(level) => json!({ "model": entry.model.key(), "thinking": level }),
            None => json!({ "model": entry.model.key() }),
        })
        .collect();
    let mut value = json!({ "name": profile.name, "chain": chain });
    if let Some(strategy) = profile.strategy {
        value["strategy"] = json!(strategy.as_str());
    }
    if let Some(exhaustion) = &profile.exhaustion {
        let policy = if exhaustion.policy == Policy::Fail {
            "fail"
        } else {
            "wait"
        };
        value["exhaustionPolicy"] = json!(policy);
        if let Some(minutes) = exhaustion.max_wait_minutes {
            value["maxWaitMinutes"] = json!(minutes);
        }
    }
    value
}

pub fn config_json(config: &Config) -> Value {
    let profiles: Map<String, Value> = config
        .profiles
        .iter()
        .map(|p| (p.id.clone(), profile_json(p)))
        .collect();
    let mut value = json!({ "profiles": profiles });
    if !config.auto_switch_on_429 {
        value["autoSwitchOn429"] = json!(false);
    }
    value
}

/// `$PI_ROUTER_CONFIG`, else `<agentDir>/ext/subscription-router.json`, else the upstream
/// `<agentDir>/subscription-router.json` when only that exists.
pub fn config_path(agent_dir: &Path) -> PathBuf {
    if let Some(path) = std::env::var_os(CONFIG_ENV).filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    let primary = agent_dir.join("ext/subscription-router.json");
    let legacy = agent_dir.join("subscription-router.json");
    if !primary.exists() && legacy.exists() {
        legacy
    } else {
        primary
    }
}

/// A missing file is an empty config.
pub fn load(path: &Path) -> Result<Config, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(err) => return Err(format!("{}: {err}", path.display())),
    };
    let raw: Value =
        serde_json::from_str(&text).map_err(|err| format!("{}: {err}", path.display()))?;
    parse_config(&raw).map_err(|err| format!("{}: {err}", path.display()))
}

/// Writes under `<agentDir>/ext/` even when the upstream file was read.
pub fn save(agent_dir: &Path, config: &Config) -> Result<PathBuf, String> {
    let path = match std::env::var_os(CONFIG_ENV).filter(|p| !p.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => agent_dir.join("ext/subscription-router.json"),
    };
    parse_config(&config_json(config))?;
    crate::fsutil::write_json(&path, &config_json(config)).map_err(|err| err.to_string())?;
    Ok(path)
}
