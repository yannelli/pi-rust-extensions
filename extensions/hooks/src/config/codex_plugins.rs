//! Hooks of enabled Codex plugins, as codex-rs/core-plugins/src/loader.rs (Codex 0.171.0) finds
//! them in `$CODEX_HOME/plugins/cache/<marketplace>/<plugin>/<version>`.

use std::collections::{BTreeMap, HashMap};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::codex::feature;
use super::codex_bundled::is_codex_bundled;
use super::codex_plugin_bundle::{active_version, bundle_trust_hash, fingerprint};
use super::options::read_json;
use super::parse_hooks;
use crate::model::{HookDef, Layer, Plugin, Scope, Source};

const LEGACY_MANIFESTS: [&str; 3] = [
    ".codex-plugin/plugin.json",
    ".claude-plugin/plugin.json",
    ".cursor-plugin/plugin.json",
];
const AGENT_SCHEMA_PREFIX: &str = "https://agent-plugins.org/schemas/";

/// Plugin hooks from `configs` (config documents, lowest precedence first). `pinned` is the
/// requirements `[features] plugins` value; `states` holds `[hooks.state]` switches.
pub fn load(
    codex_home: &Path,
    configs: &[&Value],
    pinned: Option<bool>,
    states: &HashMap<String, bool>,
) -> Vec<HookDef> {
    let on = pinned.or_else(|| configs.iter().rev().find_map(|doc| feature(doc, "plugins")));
    if on == Some(false) {
        return Vec::new();
    }
    let mut defs = Vec::new();
    for id in enabled_plugins(configs) {
        let Some((name, market)) = parse_id(&id) else {
            pi_ext_sdk::warn!("hooks: ignoring invalid Codex plugin key {id}");
            continue;
        };
        let base = codex_home.join("plugins/cache").join(market).join(name);
        let Some(version) = active_version(&base) else {
            continue;
        };
        let root = base.join(version);
        let mut plugin_defs = Vec::new();
        for (file, key_source, hooks) in hook_sources(&root) {
            for def in parse_hooks(Source::Codex, Scope::Global, &file, &hooks) {
                let file_key = def.file.display().to_string();
                let suffix = &def.codex_state_key()[file_key.len()..];
                let off = states.get(&format!("{id}:{key_source}{suffix}")) == Some(&false);
                if !off && !codex_only(&id, &def) {
                    plugin_defs.push(def);
                }
            }
        }
        if plugin_defs.is_empty() {
            continue;
        }
        let fingerprint = match fingerprint(&root) {
            Ok(fingerprint) => fingerprint,
            Err(err) => {
                pi_ext_sdk::warn!("hooks: skipping hooks of {id}: cannot fingerprint: {err}");
                continue;
            }
        };
        let data = codex_home
            .join("plugins/data")
            .join(format!("{name}-{market}"));
        let plugin = Plugin {
            id: Some(id.clone()),
            root: Some(root.display().to_string()),
            data: Some(data.display().to_string()),
            env: Vec::new(),
        };
        for mut def in plugin_defs {
            let trust = bundle_trust_hash(&def, &fingerprint);
            def.stable_hash = Some(std::mem::replace(&mut def.hash, trust));
            def.layer = Layer::Global;
            def.plugin = Some(plugin.clone());
            defs.push(def);
        }
    }
    defs
}

fn codex_only(id: &str, def: &HookDef) -> bool {
    let skip = is_codex_bundled(id, def);
    if skip {
        pi_ext_sdk::info!(
            "hooks: skipping {def}: Codex's bundled {id} hook calls a server that runs only in Codex"
        );
    }
    skip
}

/// `[plugins."<plugin>@<marketplace>"] enabled`, merged per key; `_default.enabled` fills gaps.
fn enabled_plugins(configs: &[&Value]) -> Vec<String> {
    let mut enabled: BTreeMap<String, Option<bool>> = BTreeMap::new();
    let mut default = None;
    for doc in configs {
        let plugins = doc.get("plugins").and_then(Value::as_object);
        for (key, plugin) in plugins.into_iter().flatten() {
            let value = plugin.get("enabled").and_then(Value::as_bool);
            if key == "_default" {
                default = value.or(default);
                continue;
            }
            let entry = enabled.entry(key.clone()).or_default();
            *entry = value.or(*entry);
        }
    }
    enabled
        .into_iter()
        .filter(|(_, on)| on.unwrap_or(default.unwrap_or(true)))
        .map(|(key, _)| key)
        .collect()
}

fn parse_id(id: &str) -> Option<(&str, &str)> {
    let (name, market) = id.rsplit_once('@')?;
    let word = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_');
    let name_ok = name
        .split('.')
        .all(|part| !part.is_empty() && part.chars().all(word));
    let market_ok = !market.is_empty() && market.chars().all(word);
    (name_ok && market_ok).then_some((name, market))
}

type HookSource = (PathBuf, String, Value);

/// `(file, [hooks.state] key source, hooks object)` for each hook source of the plugin.
fn hook_sources(root: &Path) -> Vec<HookSource> {
    // Codex 0.171.0 loads no hooks for a root `plugin.json` (Agent Plugins) manifest.
    let Some(Some(manifest)) = find_manifest(root) else {
        return Vec::new();
    };
    let Some(doc) = read_json(&manifest) else {
        return Vec::new();
    };
    let default = || {
        hook_file(root, &root.join("hooks/hooks.json"))
            .into_iter()
            .collect()
    };
    match doc.get("hooks") {
        None => default(),
        Some(Value::String(path)) => match resolve(root, path) {
            Some(path) => hook_file(root, &path).into_iter().collect(),
            None => default(),
        },
        Some(Value::Array(items)) if items.iter().all(Value::is_string) => {
            let paths: Vec<PathBuf> = items
                .iter()
                .filter_map(|item| resolve(root, item.as_str()?))
                .collect();
            if paths.is_empty() {
                return default();
            }
            paths
                .iter()
                .filter_map(|path| hook_file(root, path))
                .collect()
        }
        Some(hooks @ Value::Object(_)) if hooks_file(hooks) => {
            inline(&manifest, std::slice::from_ref(hooks))
        }
        Some(Value::Array(items)) if items.iter().all(hooks_file) => inline(&manifest, items),
        Some(_) => {
            pi_ext_sdk::warn!(
                "hooks: ignoring hooks in {}: unexpected type",
                manifest.display()
            );
            default()
        }
    }
}

/// The Codex `HooksFile` shape: an object with only `description` and `hooks`.
fn hooks_file(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.keys().all(|k| k == "description" || k == "hooks"))
}

fn inline(manifest: &Path, items: &[Value]) -> Vec<HookSource> {
    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let hooks = item
                .get("hooks")
                .filter(|h| h.as_object().is_some_and(|h| !h.is_empty()))?;
            let key = format!("plugin.json#hooks[{index}]");
            Some((manifest.to_path_buf(), key, hooks.clone()))
        })
        .collect()
}

fn hook_file(root: &Path, path: &Path) -> Option<HookSource> {
    let hooks = read_json(path)?.get("hooks").cloned()?;
    if hooks.as_object().is_none_or(|h| h.is_empty()) {
        return None;
    }
    let relative = path.strip_prefix(root).unwrap_or(path);
    let key = relative.to_string_lossy().replace('\\', "/");
    Some((path.to_path_buf(), key, hooks))
}

/// A `./` path that stays inside the plugin root.
fn resolve(root: &Path, path: &str) -> Option<PathBuf> {
    let relative = path.strip_prefix("./").filter(|r| !r.is_empty());
    let inside = relative.filter(|r| {
        let windows_root = cfg!(windows) && (r.starts_with('\\') || r.get(1..2) == Some(":"));
        !r.starts_with('/') && !windows_root && !r.split(['/', '\\']).any(|part| part == "..")
    });
    if inside.is_none() {
        pi_ext_sdk::warn!(
            "hooks: ignoring plugin hooks path {path}: use ./ inside the plugin root"
        );
    }
    inside.map(|r| root.join(r))
}

/// `Some(Some(path))` for a legacy manifest, `Some(None)` for a root Agent Plugins manifest.
fn find_manifest(root: &Path) -> Option<Option<PathBuf>> {
    let agent = root.join("plugin.json");
    match std::fs::symlink_metadata(&agent) {
        Ok(meta) if !meta.file_type().is_file() => return None,
        Ok(_) => {
            let doc = read_json(&agent);
            let schema = doc.as_ref().and_then(|d| d.get("$schema")?.as_str());
            if schema.is_some_and(|s| s.starts_with(AGENT_SCHEMA_PREFIX)) {
                return Some(None);
            }
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(_) => return None,
    }
    for relative in LEGACY_MANIFESTS {
        let path = root.join(relative);
        match std::fs::symlink_metadata(path.parent()?) {
            Ok(meta) if !meta.file_type().is_dir() => return None,
            Ok(_) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(_) => return None,
        }
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_file() => return Some(Some(path)),
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
#[path = "codex_plugins_tests.rs"]
mod tests;
