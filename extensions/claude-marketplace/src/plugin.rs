use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::catalog::Entry;
use crate::source::contained;

pub const UNSUPPORTED_FIELDS: [&str; 9] = [
    "lspServers",
    "monitors",
    "themes",
    "outputStyles",
    "channels",
    "userConfig",
    "bin",
    "settings",
    "workflows",
];
const UNSUPPORTED_PATHS: [&str; 6] = [
    ".lsp.json",
    "themes",
    "output-styles",
    "workflows",
    "bin",
    "settings.json",
];

/// A plugin with its component paths, all inside `root`.
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub root: PathBuf,
    pub commands: Vec<PathBuf>,
    pub skills: Vec<PathBuf>,
    pub agents: Vec<PathBuf>,
    /// Event maps (`{ "PreToolUse": [groups] }`) from each hooks source.
    pub hooks: Vec<Map<String, Value>>,
    pub mcp: Map<String, Value>,
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

fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
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

fn mcp_map(value: Value, origin: &str) -> Result<Map<String, Value>, String> {
    match value {
        Value::Object(mut map) => match map.remove("mcpServers") {
            Some(Value::Object(inner)) => Ok(inner),
            Some(_) => Err(format!("{origin}: \"mcpServers\" is not an object")),
            None => Ok(map),
        },
        _ => Err(format!("{origin}: MCP config is not an object")),
    }
}

/// Resolves the components of a plugin checked out at `root`.
pub fn resolve(entry: &Entry, root: &Path) -> Result<Resolved, String> {
    let manifest = read_manifest(root)?.unwrap_or(Value::Null);
    let sources = [&entry.raw, &manifest];
    let text = |key: &str| {
        sources
            .iter()
            .find_map(|s| s.get(key).and_then(Value::as_str).map(str::to_owned))
    };
    let mut out = Resolved {
        name: entry.name.clone(),
        version: entry.version.clone().or_else(|| text("version")),
        description: entry.description.clone().or_else(|| text("description")),
        root: root.to_path_buf(),
        ..Resolved::default()
    };
    for (kind, list) in [
        ("commands", &mut out.commands),
        ("skills", &mut out.skills),
        ("agents", &mut out.agents),
    ] {
        let declared = sources.iter().flat_map(|s| strings(s.get(kind)));
        for rel in declared.chain(std::iter::once(kind.to_owned())) {
            let path = contained(root, &rel)?;
            if !path.exists() {
                if rel != kind {
                    out.notes
                        .push(format!("{kind} path \"{rel}\" does not exist"));
                }
                continue;
            }
            if !list.contains(&path) {
                list.push(path);
            }
        }
    }
    let mut hook_files = vec![root.join("hooks/hooks.json")];
    for source in sources {
        match source.get("hooks") {
            Some(Value::Object(inline)) => out
                .hooks
                .push(event_map(Value::Object(inline.clone()), "inline hooks")?),
            other => {
                for rel in strings(other) {
                    hook_files.push(contained(root, &rel)?);
                }
            }
        }
    }
    hook_files.dedup();
    for file in hook_files.iter().filter(|f| f.is_file()) {
        let origin = file.display().to_string();
        out.hooks.push(event_map(read_json(file)?, &origin)?);
    }
    let mut mcp_files = vec![root.join(".mcp.json")];
    for source in sources {
        match source.get("mcpServers") {
            Some(Value::Object(inline)) => out.mcp.extend(inline.clone()),
            other => {
                for rel in strings(other) {
                    mcp_files.push(contained(root, &rel)?);
                }
            }
        }
    }
    mcp_files.dedup();
    for file in mcp_files.iter().filter(|f| f.is_file()) {
        let origin = file.display().to_string();
        out.mcp.extend(mcp_map(read_json(file)?, &origin)?);
    }
    for field in UNSUPPORTED_FIELDS {
        if sources.iter().any(|s| s.get(field).is_some()) {
            out.unsupported.push(field.to_owned());
        }
    }
    for path in UNSUPPORTED_PATHS {
        if root.join(path).exists() && !out.unsupported.iter().any(|u| u == path) {
            out.unsupported.push(path.to_owned());
        }
    }
    Ok(out)
}
