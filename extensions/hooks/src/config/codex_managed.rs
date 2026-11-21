//! Codex sources besides the system dir and user layers: `requirements.toml`, the cloud bundle
//! cache, the legacy `managed_config.toml`, and macOS managed preferences (MDM).

use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::Value;

use super::codex::{CodexLayer, parse_toml};
use super::codex_cloud;
use super::managed::{Requirements, codex_requirements};
use super::options::{self, Options};
use crate::paths::Paths;

const MDM_DOMAIN: &str = "com.openai.codex";
const MDM_CONFIG_KEY: &str = "config_toml_base64";
const MDM_REQUIREMENTS_KEY: &str = "requirements_toml_base64";

/// Managed Codex sources in Codex precedence order, lowest first.
#[derive(Debug, Default)]
pub struct ManagedSources {
    /// System `requirements.toml`, cloud fragments (stricter values only), then MDM.
    pub requirements: Vec<Requirements>,
    /// Cloud config fragments and requirements hooks: global layer, below the user layer.
    pub cloud_configs: Vec<CodexLayer>,
    /// Legacy `managed_config.toml`, then MDM config: above every other layer.
    pub top_configs: Vec<CodexLayer>,
}

impl ManagedSources {
    /// The sources whose switches are on.
    pub fn load(paths: &Paths, options: &Options) -> Self {
        let on = |key: &str| options.switch_on(key);
        let mut sources = Self::default();
        if let Some(system) = paths.codex_system.as_deref() {
            if on(options::CODEX_REQUIREMENTS) {
                sources.requirements.push(codex_requirements(system));
            }
            if on(options::CODEX_LEGACY)
                && let Some(layer) = legacy_config(system)
            {
                sources.top_configs.push(layer);
            }
        }
        if on(options::CODEX_CLOUD) {
            let cloud = codex_cloud::load(&paths.codex_home);
            sources.requirements.extend(cloud.requirements);
            sources.cloud_configs = cloud.configs;
        }
        if on(options::CODEX_MDM)
            && let Some(dir) = paths.codex_mdm.as_deref()
        {
            let plists = mdm_docs(&mdm_plists(dir));
            if let Some((file, doc)) = mdm_payload(&plists, MDM_REQUIREMENTS_KEY) {
                sources
                    .requirements
                    .push(Requirements::from_doc(file, &doc));
            }
            if let Some((file, doc)) = mdm_payload(&plists, MDM_CONFIG_KEY) {
                sources.top_configs.push(CodexLayer::from_config(file, doc));
            }
        }
        sources
    }

    /// The value of the highest requirements source that sets `field`.
    pub fn pin(&self, field: impl Fn(&Requirements) -> Option<bool>) -> Option<bool> {
        self.requirements.iter().rev().find_map(field)
    }
}

/// Codex reads `/etc/codex/managed_config.toml` on Unix and ignores the file on Windows.
#[cfg(unix)]
fn legacy_config(system: &Path) -> Option<CodexLayer> {
    let file = system.join("managed_config.toml");
    let doc = super::codex::read_toml(&file)?;
    Some(CodexLayer::from_config(file, doc))
}

#[cfg(not(unix))]
fn legacy_config(_system: &Path) -> Option<CodexLayer> {
    None
}

/// The per-user plist, then the device plist.
fn mdm_plists(dir: &Path) -> Vec<PathBuf> {
    let name = format!("{MDM_DOMAIN}.plist");
    let user = std::env::var("USER").ok();
    let user = user.filter(|u| !u.is_empty() && !u.contains(['/', '\\']) && u != "..");
    let user_file = user.map(|user| dir.join(user).join(&name));
    user_file.into_iter().chain([dir.join(&name)]).collect()
}

fn mdm_docs(plists: &[PathBuf]) -> Vec<(PathBuf, Value)> {
    plists
        .iter()
        .filter(|path| path.is_file())
        .filter_map(|path| plist_json(path).map(|doc| (path.clone(), doc)))
        .collect()
}

fn plist_json(path: &Path) -> Option<Value> {
    let output = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(path)
        .output();
    match output {
        Ok(out) if out.status.success() => serde_json::from_slice(&out.stdout).ok(),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            pi_ext_sdk::warn!(
                "hooks: plutil cannot read {}: {}",
                path.display(),
                stderr.trim()
            );
            None
        }
        Err(err) => {
            pi_ext_sdk::warn!("hooks: cannot run plutil for {}: {err}", path.display());
            None
        }
    }
}

/// The first plist that sets `key` wins. The value is a string of base64-encoded TOML.
fn mdm_payload(docs: &[(PathBuf, Value)], key: &str) -> Option<(PathBuf, Value)> {
    let (plist, value) = docs
        .iter()
        .find_map(|(path, doc)| Some((path, doc.get(key)?)))?;
    let name = format!("{}:{key}", plist.display());
    let Some(text) = value.as_str() else {
        pi_ext_sdk::warn!("hooks: {name} must be a string");
        return None;
    };
    let toml = BASE64
        .decode(text.trim())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok());
    let Some(toml) = toml else {
        pi_ext_sdk::warn!("hooks: {name} is not base64-encoded UTF-8");
        return None;
    };
    Some((PathBuf::from(&name), parse_toml(&toml, &name)?))
}

#[cfg(test)]
#[path = "codex_managed_tests.rs"]
mod tests;
