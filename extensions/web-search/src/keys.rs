//! API keys in `<agentDir>/ext/web-search/keys.json`, mode 0600. Env vars are the fallback.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::{Api, state_dir};

pub fn keys_path(agent_dir: &Path) -> PathBuf {
    state_dir(agent_dir).join("keys.json")
}

pub fn stored(agent_dir: &Path) -> BTreeMap<String, String> {
    let Ok(raw) = std::fs::read_to_string(keys_path(agent_dir)) else {
        return BTreeMap::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Stored,
    Env,
}

/// The stored key, else the provider env var.
pub fn api_key(agent_dir: &Path, api: Api) -> Option<(String, KeySource)> {
    let stored = stored(agent_dir)
        .remove(api.id())
        .filter(|key| !key.trim().is_empty());
    if let Some(key) = stored {
        return Some((key, KeySource::Stored));
    }
    let env = std::env::var(api.env())
        .ok()
        .filter(|key| !key.trim().is_empty());
    env.map(|key| (key, KeySource::Env))
}

/// Stores `key`, or removes the stored key when `None`.
pub fn set_key(agent_dir: &Path, api: Api, key: Option<&str>) -> std::io::Result<()> {
    let mut keys = stored(agent_dir);
    match key.map(str::trim).filter(|key| !key.is_empty()) {
        Some(key) => keys.insert(api.id().to_owned(), key.to_owned()),
        None => keys.remove(api.id()),
    };
    let path = keys_path(agent_dir);
    let dir = state_dir(agent_dir);
    std::fs::create_dir_all(&dir)?;
    let text = serde_json::to_string_pretty(&keys).map_err(std::io::Error::other)?;
    let tmp = dir.join("keys.json.tmp");
    let mut file = private_file(&tmp)?;
    file.write_all(format!("{text}\n").as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, &path)
}

#[cfg(unix)]
fn private_file(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

#[cfg(not(unix))]
fn private_file(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::create(path)
}

/// `abcd...wxyz` for display.
pub fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "*".repeat(chars.len());
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}...{tail}")
}
