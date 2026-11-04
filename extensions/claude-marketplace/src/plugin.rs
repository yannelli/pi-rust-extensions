use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::catalog::Entry;
use crate::source::contained;

/// Components pi cannot run, with the default path and the manifest key that declare them.
const UNSUPPORTED: [(&str, Option<&str>); 8] = [
    ("lspServers", Some(".lsp.json")),
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
    /// `<root>/bin` when it exists.
    pub bin: Option<PathBuf>,
    /// The `userConfig` option schema.
    pub user_config: Map<String, Value>,
    /// LSP servers (not run by pi; shown by `info`).
    pub lsp: Map<String, Value>,
    /// `agent` of the plugin settings (not applied by pi; shown by `info`).
    pub default_agent: Option<String>,
    /// `name` or `name@marketplace` of each dependency.
    pub dependencies: Vec<String>,
    pub unsupported: Vec<String>,
    pub notes: Vec<String>,
}

fn read_json(path: &Path) -> Result<Value, String> {
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

/// One declared item: a relative path or an inline object.
enum Item {
    Path(String),
    Inline(Map<String, Value>),
}

fn items(value: Option<&Value>) -> Vec<Item> {
    match value {
        Some(Value::String(s)) => vec![Item::Path(s.clone())],
        Some(Value::Object(map)) => vec![Item::Inline(map.clone())],
        Some(Value::Array(list)) => list.iter().flat_map(|v| items(Some(v))).collect(),
        _ => Vec::new(),
    }
}

fn event_map(value: Value, origin: &str) -> Result<Map<String, Value>, String> {
    let map = match value {
        Value::Object(mut map) => match map.remove("hooks") {
            Some(Value::Object(inner)) => inner,
            Some(_) => return Err(format!("{origin}: \"hooks\" is not an object")),
            None => map,
        },
        _ => return Err(format!("{origin}: hooks config is not an object")),
    };
    if let Some((event, _)) = map.iter().find(|(_, groups)| !groups.is_array()) {
        return Err(format!("{origin}: hooks for {event} are not a list"));
    }
    Ok(map)
}

fn server_map(value: Value, key: &str, origin: &str) -> Result<Map<String, Value>, String> {
    match value {
        Value::Object(mut map) => match map.remove(key) {
            Some(Value::Object(inner)) => Ok(inner),
            Some(_) => Err(format!("{origin}: \"{key}\" is not an object")),
            None => Ok(map),
        },
        _ => Err(format!("{origin}: config is not an object")),
    }
}

/// Reads a server map (`mcpServers` or `lspServers`): the default file, then the declared items.
fn servers(
    root: &Path,
    default: &str,
    key: &str,
    value: Option<&Value>,
    notes: &mut Vec<String>,
) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    let mut files = vec![root.join(default)];
    for item in items(value) {
        match item {
            Item::Inline(map) => out.extend(server_map(Value::Object(map), key, "inline")?),
            Item::Path(rel) if rel.starts_with("https://") || !rel.ends_with(".json") => {
                notes.push(format!("{key} \"{rel}\" is not a JSON file; skipped"));
            }
            Item::Path(rel) => files.push(contained(root, &rel)?),
        }
    }
    files.dedup();
    for file in files.iter().filter(|f| f.is_file()) {
        let origin = file.display().to_string();
        out.extend(server_map(read_json(file)?, key, &origin)?);
    }
    Ok(out)
}

fn text_of(map: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| map.get(*k))
        .map(|v| match v {
            Value::Array(list) => {
                let names: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
                names.join(", ")
            }
            other => other.as_str().map(str::to_owned).unwrap_or_default(),
        })
        .filter(|s| !s.is_empty())
}

fn inline_commands(
    root: &Path,
    map: &Map<String, Value>,
    out: &mut Resolved,
) -> Result<(), String> {
    for (name, spec) in map {
        let Value::Object(spec) = spec else {
            out.notes
                .push(format!("command \"{name}\" is not an object"));
            continue;
        };
        let source = match spec.get("source").and_then(Value::as_str) {
            Some(rel) => Some(contained(root, rel)?),
            None => None,
        };
        let content = spec
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if source.is_none() == content.is_none() {
            out.notes
                .push(format!("command \"{name}\" needs one of source or content"));
            continue;
        }
        out.inline_commands.push(InlineCommand {
            name: name.clone(),
            source,
            content,
            description: text_of(spec, &["description"]),
            argument_hint: text_of(spec, &["argumentHint", "argument-hint"]),
            model: text_of(spec, &["model"]),
            allowed_tools: text_of(spec, &["allowedTools", "allowed-tools"]),
        });
    }
    Ok(())
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

/// Resolves the components of a plugin checked out at `root`.
pub fn resolve(entry: &Entry, root: &Path) -> Result<Resolved, String> {
    let manifest = read_manifest(root)?;
    let strict = entry.raw.get("strict").and_then(Value::as_bool) != Some(false);
    let entry_declares = COMPONENT_KEYS.iter().any(|k| entry.raw.get(*k).is_some());
    if manifest.is_some() && !strict && entry_declares {
        return Err(format!(
            "{} has conflicting manifests: the marketplace entry sets strict: false and declares components, and the plugin has a plugin.json",
            entry.name
        ));
    }
    let manifest = manifest.unwrap_or(Value::Null);
    let has_manifest = !manifest.is_null();
    let all: Vec<&Value> = if has_manifest {
        vec![&manifest, &entry.raw]
    } else {
        vec![&entry.raw]
    };
    let own: Vec<&Value> = if has_manifest {
        vec![&manifest]
    } else {
        vec![&entry.raw]
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
        ..Resolved::default()
    };
    if has_manifest {
        for key in MANIFEST_ONLY {
            if entry.raw.get(key).is_some() {
                out.notes.push(format!(
                    "the marketplace entry's {key} is ignored because the plugin has a plugin.json"
                ));
            }
        }
    }
    resolve_paths(root, &all, &mut out)?;
    resolve_hooks(root, &own, has_manifest, &entry.raw, &mut out)?;
    let mcp_value = own.iter().find_map(|s| s.get("mcpServers"));
    out.mcp = servers(root, ".mcp.json", "mcpServers", mcp_value, &mut out.notes)?;
    let lsp_value = own.iter().find_map(|s| s.get("lspServers"));
    out.lsp = servers(root, ".lsp.json", "lspServers", lsp_value, &mut out.notes)?;
    if let Some(Value::Object(options)) = own.iter().find_map(|s| s.get("userConfig")) {
        out.user_config = options.clone();
    }
    let bin = root.join("bin");
    if bin.is_dir() {
        out.bin = Some(bin);
    }
    out.default_agent = default_agent(root, &all);
    for item in all
        .iter()
        .flat_map(|s| dependency_ids(s.get("dependencies")))
    {
        if !out.dependencies.contains(&item) {
            out.dependencies.push(item);
        }
    }
    for (component, path) in UNSUPPORTED {
        let present =
            !declared(&all, component).is_empty() || path.is_some_and(|p| root.join(p).exists());
        if present {
            out.unsupported.push(component.to_owned());
        }
    }
    Ok(out)
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
                    out.notes.push(format!("{key} has an object; skipped"));
                    continue;
                }
            };
            declared_any = true;
            let path = contained(root, &rel)?;
            if !path.exists() {
                out.notes
                    .push(format!("{key} path \"{rel}\" does not exist"));
            } else if !paths.contains(&path) {
                paths.push(path);
            }
        }
        let default_path = root.join(default);
        if declared_any && replaces {
            if default_path.is_dir() && !paths.contains(&default_path) {
                out.notes.push(format!(
                    "the default {default}/ folder is ignored because the manifest sets {key}"
                ));
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

/// Hooks merge across sources. With a plugin.json, an event in the strict entry's inline hooks
/// replaces the groups of that event from the plugin.
fn resolve_hooks(
    root: &Path,
    own: &[&Value],
    has_manifest: bool,
    entry: &Value,
    out: &mut Resolved,
) -> Result<(), String> {
    let mut files = vec![root.join("hooks/hooks.json")];
    let mut inline: Vec<Map<String, Value>> = Vec::new();
    for item in own.iter().flat_map(|s| items(s.get("hooks"))) {
        match item {
            Item::Inline(map) => inline.push(event_map(Value::Object(map), "inline hooks")?),
            Item::Path(rel) => files.push(contained(root, &rel)?),
        }
    }
    files.dedup();
    let mut maps: Vec<Map<String, Value>> = Vec::new();
    for file in files.iter().filter(|f| f.is_file()) {
        let origin = file.display().to_string();
        maps.push(event_map(read_json(file)?, &origin)?);
    }
    maps.extend(inline);
    if has_manifest {
        for item in items(entry.get("hooks")) {
            let Item::Inline(map) = item else {
                out.notes
                    .push("the marketplace entry's hooks must be inline; skipped".into());
                continue;
            };
            let map = event_map(Value::Object(map), "marketplace entry hooks")?;
            for earlier in maps.iter_mut() {
                earlier.retain(|event, _| !map.contains_key(event));
            }
            maps.push(map);
        }
    }
    out.hooks = maps.into_iter().filter(|m| !m.is_empty()).collect();
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

fn dependency_ids(value: Option<&Value>) -> Vec<String> {
    let list = match value {
        Some(Value::Array(list)) => list.as_slice(),
        Some(one) => std::slice::from_ref(one),
        None => &[],
    };
    list.iter()
        .filter_map(|dep| match dep {
            Value::String(id) => Some(id.clone()),
            Value::Object(map) => {
                let name = map.get("name")?.as_str()?;
                Some(match map.get("marketplace").and_then(Value::as_str) {
                    Some(mp) => format!("{name}@{mp}"),
                    None => name.to_owned(),
                })
            }
            _ => None,
        })
        .collect()
}
