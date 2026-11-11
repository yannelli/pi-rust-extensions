//! `<agent dir>/ext/hybrid-router.json`: the local model, the remote model, and the busy limit.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_BUSY_LOAD_PER_CORE: f64 = 0.8;

/// A model written `provider/id`; the id may contain `/`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ModelRef {
    pub provider: String,
    pub id: String,
}

impl ModelRef {
    pub fn parse(text: &str) -> Option<Self> {
        let (provider, id) = text.split_once('/')?;
        let valid = !provider.is_empty() && !id.is_empty();
        valid.then(|| Self {
            provider: provider.into(),
            id: id.into(),
        })
    }

    /// The `provider` and `id` of a model object.
    pub fn of(model: &Value) -> Option<Self> {
        let provider = model["provider"].as_str()?;
        let id = model["id"].as_str()?;
        Some(Self {
            provider: provider.into(),
            id: id.into(),
        })
    }
}

impl fmt::Display for ModelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.provider, self.id)
    }
}

impl TryFrom<String> for ModelRef {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        Self::parse(&text).ok_or_else(|| format!("\"{text}\" is not provider/id"))
    }
}

impl From<ModelRef> for String {
    fn from(model: ModelRef) -> Self {
        model.to_string()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// A chat model with a classifier of the same provider and id, such as a llama.cpp model.
    pub local: ModelRef,
    pub remote: ModelRef,
    /// Above this 1-minute load average per core, prompts go to the remote model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy_load_per_core: Option<f64>,
}

impl Config {
    pub fn busy(&self, load_per_core: f64) -> bool {
        load_per_core
            > self
                .busy_load_per_core
                .unwrap_or(DEFAULT_BUSY_LOAD_PER_CORE)
    }
}

pub fn path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join("hybrid-router.json")
}

/// The saved config; `Err` when the file exists and does not parse.
pub fn load(agent_dir: &Path) -> Result<Option<Config>, String> {
    let path = path(agent_dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| format!("{}: {error}", path.display()))
}

pub fn save(agent_dir: &Path, config: &Config) -> Result<PathBuf, String> {
    let path = path(agent_dir);
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap_or(agent_dir))?;
        let text = serde_json::to_string_pretty(config).map_err(std::io::Error::other)?;
        std::fs::write(&path, format!("{text}\n"))
    };
    write().map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(path)
}

pub fn remove(agent_dir: &Path) -> Result<(), String> {
    let path = path(agent_dir);
    match std::fs::remove_file(&path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("cannot remove {}: {error}", path.display()))
        }
        _ => Ok(()),
    }
}
