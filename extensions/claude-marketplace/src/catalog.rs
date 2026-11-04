use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::source::{PluginSource, check_name, contained, parse_plugin};
use crate::synthetic;

/// A parsed `.claude-plugin/marketplace.json`.
#[derive(Debug, Clone)]
pub struct Catalog {
    pub name: String,
    pub root: PathBuf,
    pub plugin_root: Option<String>,
    pub plugins: Vec<Entry>,
    /// The whole marketplace.json.
    pub raw: Value,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub source: PluginSource,
    pub raw: Value,
}

impl Catalog {
    /// Reads `manifest`; without it, a plugin or skill layout at `root` becomes a
    /// marketplace with one plugin, named after `name` or the root dir.
    pub fn read_named(manifest: &Path, root: &Path, name: Option<&str>) -> Result<Catalog> {
        if !manifest.is_file() {
            let dir = root.file_name().map(|n| n.to_string_lossy().into_owned());
            let name = synthetic::name_from(name.or(dir.as_deref()).unwrap_or_default());
            if let Some((value, _)) = synthetic::catalog(root, &name) {
                return Catalog::from_value(&value, root);
            }
            anyhow::bail!(
                "{} has no .claude-plugin/marketplace.json, plugin.json, or SKILL.md",
                root.display()
            );
        }
        Catalog::read(manifest, root)
    }

    pub fn read(manifest: &Path, root: &Path) -> Result<Catalog> {
        if !manifest.is_file() {
            return Catalog::read_named(manifest, root, None);
        }
        let text = std::fs::read_to_string(manifest)
            .with_context(|| format!("cannot read {}", manifest.display()))?;
        let value: Value = serde_json::from_str(&text)
            .with_context(|| format!("{} is not valid JSON", manifest.display()))?;
        Catalog::from_value(&value, root)
    }

    pub fn from_value(value: &Value, root: &Path) -> Result<Catalog> {
        let Some(name) = value.get("name").and_then(Value::as_str) else {
            bail!("marketplace.json has no \"name\"");
        };
        check_name("marketplace", name)?;
        let plugin_root = value
            .pointer("/metadata/pluginRoot")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let mut plugins = Vec::new();
        let entries = value.get("plugins").and_then(Value::as_array);
        for raw in entries.into_iter().flatten() {
            let Some(name) = raw.get("name").and_then(Value::as_str) else {
                pi_ext_sdk::warn!("skipping a marketplace entry without a name");
                continue;
            };
            if let Err(err) = check_name("plugin", name) {
                pi_ext_sdk::warn!("skipping marketplace entry: {err}");
                continue;
            }
            let text = |key: &str| raw.get(key).and_then(Value::as_str).map(str::to_owned);
            let source = match raw.get("source") {
                Some(source) => parse_plugin(source),
                None => PluginSource::Unsupported("entry has no source".into()),
            };
            plugins.push(Entry {
                name: name.to_owned(),
                description: text("description"),
                version: text("version"),
                source,
                raw: raw.clone(),
            });
        }
        Ok(Catalog {
            name: name.to_owned(),
            root: root.to_path_buf(),
            plugin_root,
            plugins,
            raw: value.clone(),
        })
    }

    pub fn entry(&self, plugin: &str) -> Option<&Entry> {
        self.plugins.iter().find(|entry| entry.name == plugin)
    }

    /// The directory of a relative-path plugin, kept inside the marketplace root.
    pub fn local_root(&self, rel: &str) -> Result<PathBuf, String> {
        let base = match &self.plugin_root {
            Some(base) => contained(&self.root, base)?,
            None => self.root.clone(),
        };
        let root = contained(&base, rel)?;
        let escapes = match (root.canonicalize(), self.root.canonicalize()) {
            (Ok(real), Ok(real_root)) => !real.starts_with(real_root),
            _ => false,
        };
        if escapes {
            return Err(format!("plugin source \"{rel}\" leaves the marketplace"));
        }
        if !root.is_dir() {
            return Err(format!("plugin directory \"{rel}\" does not exist"));
        }
        Ok(root)
    }
}
