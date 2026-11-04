use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::catalog::Entry;
use crate::components::{
    Item, event_map, inline_commands, items, resolve_hooks, server_map, servers,
};
use crate::fields::{self, Dropped};
use crate::settings::Settings;
use crate::source::contained;

/// Components pi cannot run, with the default path and the manifest key that declare them.
const UNSUPPORTED: [(&str, Option<&str>); 7] = [
    ("monitors", Some("monitors/monitors.json")),
    ("themes", Some("themes")),
    ("channels", None),
    ("workflows", Some("workflows")),
    ("settings", Some("settings.json")),
    ("evals", Some("evals")),
    ("types", None),
];

/// Manifest keys that declare components. A `strict: false` entry with one of them conflicts
/// with a plugin.json.
const COMPONENT_KEYS: [&str; 14] = [
    "commands",
    "agents",
    "skills",
    "hooks",
    "mcpServers",
    "outputStyles",
    "lspServers",
    "monitors",
    "themes",
    "channels",
    "userConfig",
    "settings",
    "workflows",
    "experimental",
];

/// Keys that a strict marketplace entry cannot add to a plugin.json.
const MANIFEST_ONLY: [&str; 4] = ["mcpServers", "lspServers", "userConfig", "channels"];

/// A command from the object form of `commands`: `{ "<name>": { source | content, ... } }`.
#[derive(Debug, Clone, Default)]
pub struct InlineCommand {
    pub name: String,
    pub source: Option<PathBuf>,
    pub content: Option<String>,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
    pub model: Option<String>,
    pub allowed_tools: Option<String>,
}

/// A plugin with its component paths, all inside `root`.
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub root: PathBuf,
    pub commands: Vec<PathBuf>,
    pub inline_commands: Vec<InlineCommand>,
    pub skills: Vec<PathBuf>,
    pub agents: Vec<PathBuf>,
    pub output_styles: Vec<PathBuf>,
    /// Event maps (`{ "PreToolUse": [groups] }`) from each hooks source.
    pub hooks: Vec<Map<String, Value>>,
    pub mcp: Map<String, Value>,
    /// `<root>/bin` when it exists, or the config `paths.bin`, then `extraPaths.bin`.
    pub bins: Vec<PathBuf>,
    /// The `userConfig` option schema.
    pub user_config: Map<String, Value>,
    /// LSP servers, staged in the lens server file.
    pub lsp: Map<String, Value>,
    /// `agent` of the plugin settings (not applied by pi; shown by `info`).
    pub default_agent: Option<String>,
    /// `name` or `name@marketplace` of each dependency.
    pub dependencies: Vec<String>,
    pub unsupported: Vec<String>,
    /// Components and fields that are not applied, with the reason.
    pub dropped: Vec<Dropped>,
    /// The effective `strict` and where it comes from.
    pub strict: String,
}

pub(crate) fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {e}", path.display()))
}

pub fn read_manifest(root: &Path) -> Result<Option<Value>, String> {
    let candidates = [
        root.join(".claude-plugin/plugin.json"),
        root.join("plugin.json"),
    ];
    match candidates.iter().find(|path| path.is_file()) {
        Some(path) => match read_json(path)? {
            Value::Object(map) => Ok(Some(Value::Object(map))),
            _ => Err(format!("{} is not a JSON object", path.display())),
        },
        None => Ok(None),
    }
}

/// The values of `key` in the sources, directly or under `experimental`.
fn declared<'a>(sources: &[&'a Value], key: &str) -> Vec<&'a Value> {
    let mut out: Vec<&Value> = sources.iter().filter_map(|s| s.get(key)).collect();
    let nested = sources
        .iter()
        .filter_map(|s| s.pointer(&format!("/experimental/{key}")));
    out.extend(nested);
    out
}

/// Resolves the components of a plugin checked out at `root`, with no config overrides.
pub fn resolve(entry: &Entry, root: &Path) -> Result<Resolved, String> {
    resolve_with(entry, root, &Settings::default())
}

/// `paths.<kind>` of the config replaces the declaration of that kind in both manifests.
fn override_paths(own: &mut Value, other: &mut Value, settings: &Settings) {
    for (kind, value) in &settings.paths {
        if kind == "bin" {
            continue;
        }
        if let Some(map) = other.as_object_mut() {
            map.remove(kind);
        }
        if let Some(map) = own.as_object_mut() {
            map.insert(kind.clone(), value.clone());
        }
    }
}

/// Resolves the components of a plugin checked out at `root`, with the config `settings`.
pub fn resolve_with(entry: &Entry, root: &Path, settings: &Settings) -> Result<Resolved, String> {
    let manifest = read_manifest(root)?;
    let (strict, strict_from) = match (&settings.strict, entry.raw.get("strict")) {
        (Some((value, label)), _) => (*value, label.clone()),
        (None, Some(value)) => (value.as_bool() != Some(false), "marketplace entry".into()),
        (None, None) => (true, "default".into()),
    };
    let entry_declares = COMPONENT_KEYS.iter().any(|k| entry.raw.get(*k).is_some());
    if manifest.is_some() && !strict && entry_declares {
        return Err(format!(
            "{} has conflicting manifests: strict is false ({strict_from}), the marketplace entry declares components, and the plugin has a plugin.json",
            entry.name
        ));
    }
    let has_manifest = manifest.is_some();
    let mut dropped = Vec::new();
    let mut entry_raw = entry.raw.clone();
    if let Some(manifest) = &manifest {
        fields::manifest(manifest, "plugin.json", &mut dropped);
        for key in MANIFEST_ONLY {
            if let Some(map) = entry_raw.as_object_mut().filter(|m| m.contains_key(key)) {
                map.remove(key);
                let reason = "ignored because the plugin has a plugin.json";
                dropped.push(Dropped::new(format!("marketplace entry {key}"), reason));
            }
        }
    }
    fields::manifest(&entry_raw, "marketplace entry", &mut dropped);
    let mut manifest = manifest.unwrap_or(Value::Null);
    match has_manifest {
        true => override_paths(&mut manifest, &mut entry_raw, settings),
        false => override_paths(&mut entry_raw, &mut Value::Null, settings),
    }
    let all: Vec<&Value> = match has_manifest {
        true => vec![&manifest, &entry_raw],
        false => vec![&entry_raw],
    };
    let own: Vec<&Value> = match has_manifest {
        true => vec![&manifest],
        false => vec![&entry_raw],
    };
    let text = |key: &str| {
        own.iter()
            .chain(all.iter())
            .find_map(|s| s.get(key).and_then(Value::as_str).map(str::to_owned))
    };
    let mut out = Resolved {
        name: entry.name.clone(),
        version: text("version").or_else(|| entry.version.clone()),
        description: entry.description.clone().or_else(|| text("description")),
        root: root.to_path_buf(),
        dropped,
        strict: format!("{strict} ({strict_from})"),
        ..Resolved::default()
    };
    resolve_paths(root, &all, &mut out)?;
    resolve_hooks(root, &own, has_manifest, &entry_raw, &mut out)?;
    let mcp_value = own.iter().find_map(|s| s.get("mcpServers"));
    out.mcp = servers(root, ".mcp.json", "mcpServers", mcp_value, &mut out.dropped)?;
    let lsp_value = own.iter().find_map(|s| s.get("lspServers"));
    out.lsp = servers(root, ".lsp.json", "lspServers", lsp_value, &mut out.dropped)?;
    if let Some(Value::Object(options)) = own.iter().find_map(|s| s.get("userConfig")) {
        out.user_config = options.clone();
    }
    match settings.paths.get("bin") {
        Some(value) => out.bins = config_paths(root, value, "paths.bin")?,
        None => out
            .bins
            .extend(Some(root.join("bin")).filter(|b| b.is_dir())),
    }
    out.default_agent = default_agent(root, &all);
    for source in &all {
        for (id, version) in dependency_ids(source.get("dependencies")) {
            if let Some(range) = version {
                let reason = "pi installs the dependency but does not resolve version ranges";
                out.dropped.push(Dropped::new(
                    format!("dependency {id} version {range}"),
                    reason,
                ));
            }
            if !out.dependencies.contains(&id) {
                out.dependencies.push(id);
            }
        }
    }
    for (component, path) in UNSUPPORTED {
        let file = path.filter(|p| root.join(p).exists());
        if let (Some(file), Some(reason)) = (file, fields::reason(component)) {
            let what = match root.join(file).is_dir() {
                true => format!("{file}/"),
                false => file.to_owned(),
            };
            out.dropped.push(Dropped::new(what, reason));
        }
        if !declared(&all, component).is_empty() || file.is_some() {
            out.unsupported.push(component.to_owned());
        }
    }
    extra_paths(root, settings, &mut out)?;
    Ok(out)
}

/// Relative paths of a config value (a string or a list), kept inside the plugin root.
fn config_paths(root: &Path, value: &Value, what: &str) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for item in items(Some(value)) {
        let Item::Path(rel) = item else {
            return Err(format!("config {what} must be paths"));
        };
        let path = contained(root, &rel).map_err(|e| format!("config {what}: {e}"))?;
        if !path.exists() {
            return Err(format!("config {what}: \"{rel}\" does not exist"));
        }
        out.push(path);
    }
    Ok(out)
}

/// `extraPaths.<kind>`: added after the manifest rules, so default folders stay.
fn extra_paths(root: &Path, settings: &Settings, out: &mut Resolved) -> Result<(), String> {
    for (kind, value) in &settings.extra_paths {
        let paths = config_paths(root, value, &format!("extraPaths.{kind}"))?;
        for path in paths {
            let origin = path.display().to_string();
            let list = match kind.as_str() {
                "commands" => &mut out.commands,
                "agents" => &mut out.agents,
                "skills" => &mut out.skills,
                "outputStyles" => &mut out.output_styles,
                "bin" => &mut out.bins,
                "hooks" => {
                    out.hooks.push(event_map(read_json(&path)?, &origin)?);
                    continue;
                }
                "lspServers" => {
                    out.lsp
                        .extend(server_map(read_json(&path)?, "lspServers", &origin)?);
                    continue;
                }
                _ => {
                    out.mcp
                        .extend(server_map(read_json(&path)?, "mcpServers", &origin)?);
                    continue;
                }
            };
            if !list.contains(&path) {
                list.push(path);
            }
        }
    }
    Ok(())
}

/// Commands, agents, skills, and output styles. A declared path replaces the default folder,
/// except for skills, where it adds to `skills/`.
fn resolve_paths(root: &Path, all: &[&Value], out: &mut Resolved) -> Result<(), String> {
    let kinds = [
        ("commands", "commands", true),
        ("agents", "agents", true),
        ("skills", "skills", false),
        ("outputStyles", "output-styles", true),
    ];
    for (key, default, replaces) in kinds {
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut declared_any = false;
        for item in all.iter().flat_map(|s| items(s.get(key))) {
            let rel = match item {
                Item::Path(rel) => rel,
                Item::Inline(map) if key == "commands" => {
                    declared_any = true;
                    inline_commands(root, &map, out)?;
                    continue;
                }
                Item::Inline(_) => {
                    let reason = "only commands take the object form";
                    out.dropped
                        .push(Dropped::new(format!("{key} object"), reason));
                    continue;
                }
            };
            declared_any = true;
            let path = contained(root, &rel)?;
            if !path.exists() {
                let reason = "the path does not exist";
                out.dropped
                    .push(Dropped::new(format!("{key} \"{rel}\""), reason));
            } else if !paths.contains(&path) {
                paths.push(path);
            }
        }
        let default_path = root.join(default);
        if declared_any && replaces {
            if default_path.is_dir() && !paths.contains(&default_path) {
                let reason = format!("the manifest sets {key}, which replaces the default folder");
                out.dropped
                    .push(Dropped::new(format!("{default}/ folder"), reason));
            }
        } else if default_path.exists() && !paths.contains(&default_path) {
            paths.push(default_path);
        }
        let single_skill = key == "skills" && !declared_any && root.join("SKILL.md").is_file();
        if single_skill && paths.is_empty() {
            paths.push(root.to_path_buf());
        }
        match key {
            "commands" => out.commands = paths,
            "agents" => out.agents = paths,
            "skills" => out.skills = paths,
            _ => out.output_styles = paths,
        }
    }
    Ok(())
}

fn default_agent(root: &Path, all: &[&Value]) -> Option<String> {
    let file = read_json(&root.join("settings.json")).ok();
    let from_file = file.as_ref().and_then(|v| v.get("agent")?.as_str());
    let from_key = all
        .iter()
        .find_map(|s| s.pointer("/settings/agent")?.as_str());
    from_file.or(from_key).map(str::to_owned)
}

/// `(id, version range)` of each dependency.
fn dependency_ids(value: Option<&Value>) -> Vec<(String, Option<String>)> {
    let list = match value {
        Some(Value::Array(list)) => list.as_slice(),
        Some(one) => std::slice::from_ref(one),
        None => &[],
    };
    list.iter()
        .filter_map(|dep| match dep {
            Value::String(id) => Some((id.clone(), None)),
            Value::Object(map) => {
                let name = map.get("name")?.as_str()?;
                let id = match map.get("marketplace").and_then(Value::as_str) {
                    Some(mp) => format!("{name}@{mp}"),
                    None => name.to_owned(),
                };
                let version = map
                    .get("version")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                Some((id, version))
            }
            _ => None,
        })
        .collect()
}
