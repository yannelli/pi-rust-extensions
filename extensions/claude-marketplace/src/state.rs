use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::env::{Dirs, Scope};
use crate::jsonfile;
use crate::source::check_name;

/// `<scopeRoot>/ext/claude-marketplace/state.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    pub marketplaces: BTreeMap<String, MarketplaceRecord>,
    pub plugins: BTreeMap<String, PluginRecord>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MarketplaceRecord {
    pub name: String,
    /// The source as the user gave it (absolute for local paths).
    pub source: String,
    /// Clone URL; `None` for a local directory read in place.
    pub git_url: Option<String>,
    pub root: PathBuf,
    pub manifest: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRecord {
    pub plugin: String,
    pub marketplace: String,
    pub marketplace_scope: Scope,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub enabled: bool,
    pub plugin_root: PathBuf,
    /// Staged prompt files, skill dirs, and agent files.
    #[serde(default)]
    pub files: Vec<PathBuf>,
    #[serde(default)]
    pub hooks: bool,
    #[serde(default)]
    pub mcp_keys: Vec<String>,
    #[serde(default)]
    pub unsupported: Vec<String>,
}

impl State {
    pub fn load(dirs: &Dirs) -> Result<State> {
        let path = dirs.state_file();
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("{} is not a valid state file", path.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(err) => Err(err).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    pub fn save(&self, dirs: &Dirs) -> Result<()> {
        jsonfile::write(&dirs.state_file(), &serde_json::to_value(self)?)
    }
}

pub fn plugin_id(plugin: &str, marketplace: &str) -> String {
    format!("{plugin}@{marketplace}")
}

/// Splits `plugin@marketplace`.
pub fn split_id(id: &str) -> Result<(&str, &str)> {
    match id.rsplit_once('@') {
        Some((plugin, marketplace)) if !plugin.is_empty() && !marketplace.is_empty() => {
            check_name("plugin", plugin)?;
            check_name("marketplace", marketplace)?;
            Ok((plugin, marketplace))
        }
        _ => anyhow::bail!("expected <plugin>@<marketplace>, got \"{id}\""),
    }
}
