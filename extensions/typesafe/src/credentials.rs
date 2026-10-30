use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::backends::{Backend, DEFAULT_BACKEND};
use crate::errors::{TsError, TsResult};

/// Environment lookups; tests pass a fixed map instead of the process environment.
#[derive(Debug, Clone, Default)]
pub struct Env(Option<HashMap<String, String>>);

impl Env {
    pub fn process() -> Self {
        Self(None)
    }

    pub fn fixed<K: Into<String>, V: Into<String>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        Self(Some(
            pairs
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        ))
    }

    /// The trimmed value, or `None` when unset or blank.
    pub fn get(&self, key: &str) -> Option<String> {
        let raw = match &self.0 {
            Some(map) => map.get(key).cloned(),
            None => std::env::var(key).ok(),
        };
        raw.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
    }
}

/// Writes `text` owner-only under a temporary name, then renames it over `path`.
pub fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        create_private_dir(dir)?;
    }
    let temporary = PathBuf::from(format!("{}.{}.tmp", path.display(), std::process::id()));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temporary)?;
        file.write_all(text.as_bytes())?;
        #[cfg(unix)]
        fs::set_permissions(
            &temporary,
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

#[cfg(unix)]
fn readable_by_others(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o077 != 0)
}

#[cfg(not(unix))]
fn readable_by_others(_path: &Path) -> bool {
    false
}

/// Where a key came from, as upstream `keySituation`.
#[derive(Debug, Clone, PartialEq)]
pub enum KeySituation {
    Environment { key: String, key_env: &'static str },
    Stored { key: String },
    Missing,
    Unusable { reason: String },
}

impl KeySituation {
    pub fn key(&self) -> Option<&str> {
        match self {
            KeySituation::Environment { key, .. } | KeySituation::Stored { key } => Some(key),
            _ => None,
        }
    }

    /// Short phrase naming the source.
    pub fn label(&self) -> &str {
        match self {
            KeySituation::Environment { key_env, .. } => key_env,
            KeySituation::Stored { .. } => "/typesafe login",
            KeySituation::Missing => "no key",
            KeySituation::Unusable { .. } => "unusable key",
        }
    }
}

/// Accepts a plausible token only; the message never echoes the value.
pub fn normalize_key(value: &str) -> TsResult<String> {
    let key = value.trim();
    let printable = key.bytes().all(|b| (0x21..=0x7e).contains(&b));
    if key.len() < 16 || key.len() > 512 || !printable {
        return Err(TsError::validation(
            "That does not look like a TypeSafe API key. Copy the complete key from console.typesafe.ai and try again; nothing was saved.",
        ));
    }
    Ok(key.to_owned())
}

/// `<agentDir>/pi-typesafe/auth.json`: `{ version: 1, keys: { <backend>: key } }`, mode 0600.
#[derive(Debug, Clone)]
pub struct KeyStore {
    pub path: PathBuf,
}

impl KeyStore {
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join("auth.json"),
        }
    }

    fn read_keys(&self) -> std::io::Result<Map<String, Value>> {
        let raw: Value = serde_json::from_str(&fs::read_to_string(&self.path)?)?;
        let mut keys = Map::new();
        if let Some(stored) = raw.get("keys").and_then(Value::as_object) {
            for (id, value) in stored {
                if let Some(key) = value.as_str().map(str::trim).filter(|k| !k.is_empty()) {
                    keys.insert(id.clone(), json!(key));
                }
            }
        }
        if let Some(legacy) = raw.get("apiKey").and_then(Value::as_str).map(str::trim) {
            if !legacy.is_empty() {
                keys.insert(DEFAULT_BACKEND.to_owned(), json!(legacy));
            }
        }
        Ok(keys)
    }

    fn write_keys(&self, keys: Map<String, Value>) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(&json!({ "version": 1, "keys": keys }))?;
        write_private(&self.path, &format!("{text}\n"))
    }

    /// The stored key, refusing a file that other users can read.
    pub fn read(&self, backend: &str) -> TsResult<Option<String>> {
        if readable_by_others(&self.path) {
            return Err(TsError::configuration(format!(
                "Refusing to read {}: it is readable by other users. Run chmod 600 on it, or run /typesafe logout and /typesafe login.",
                self.path.display()
            )));
        }
        let keys = self.read_keys().unwrap_or_default();
        Ok(keys.get(backend).and_then(Value::as_str).map(str::to_owned))
    }

    /// The backend's own variable, then its stored key, then missing.
    pub fn situation(&self, backend: &Backend, env: &Env) -> KeySituation {
        if let Some(key) = env.get(backend.key_env) {
            return KeySituation::Environment {
                key,
                key_env: backend.key_env,
            };
        }
        match self.read(backend.name) {
            Ok(Some(key)) => KeySituation::Stored { key },
            Ok(None) => KeySituation::Missing,
            Err(error) => KeySituation::Unusable {
                reason: error.message,
            },
        }
    }

    /// Saves one backend's key; an unreadable store is replaced.
    pub fn store(&self, value: &str, backend: &Backend) -> TsResult<PathBuf> {
        let key = normalize_key(value)?;
        let mut keys = self.read_keys().unwrap_or_default();
        keys.insert(backend.name.to_owned(), json!(key));
        self.write_keys(keys).map_err(|_| {
            TsError::configuration(format!(
                "Could not write {}. Check directory permissions, or set {} in the environment instead.",
                self.path.display(),
                backend.key_env
            ))
        })?;
        Ok(self.path.clone())
    }

    /// Removes one backend's key. Returns whether a key was removed.
    pub fn clear(&self, backend: &str) -> bool {
        if !self.path.exists() {
            return false;
        }
        let Ok(mut keys) = self.read_keys() else {
            let _ = fs::remove_file(&self.path);
            return true;
        };
        if keys.remove(backend).is_none() {
            return false;
        }
        if keys.is_empty() {
            let _ = fs::remove_file(&self.path);
        } else {
            let _ = self.write_keys(keys);
        }
        true
    }
}

#[cfg(test)]
#[path = "credentials_tests.rs"]
mod tests;
