//! Parsing of declared component items: hooks, MCP and LSP servers, and inline commands.

use std::path::Path;

use serde_json::{Map, Value};

use crate::fields::Dropped;
use crate::plugin::{InlineCommand, Resolved, read_json};
use crate::source::contained;

/// One declared item: a relative path or an inline object.
pub enum Item {
    Path(String),
    Inline(Map<String, Value>),
}

pub fn items(value: Option<&Value>) -> Vec<Item> {
    match value {
        Some(Value::String(s)) => vec![Item::Path(s.clone())],
        Some(Value::Object(map)) => vec![Item::Inline(map.clone())],
        Some(Value::Array(list)) => list.iter().flat_map(|v| items(Some(v))).collect(),
        _ => Vec::new(),
    }
}

pub fn event_map(value: Value, origin: &str) -> Result<Map<String, Value>, String> {
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

pub fn server_map(value: Value, key: &str, origin: &str) -> Result<Map<String, Value>, String> {
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
pub fn servers(
    root: &Path,
    default: &str,
    key: &str,
    value: Option<&Value>,
    dropped: &mut Vec<Dropped>,
) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    let mut files = vec![root.join(default)];
    for item in items(value) {
        match item {
            Item::Inline(map) => out.extend(server_map(Value::Object(map), key, "inline")?),
            Item::Path(rel) if rel.starts_with("https://") || !rel.ends_with(".json") => {
                let reason = "pi reads only JSON server files, not .mcpb, .dxt, or URLs";
                dropped.push(Dropped::new(format!("{key} \"{rel}\""), reason));
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

pub fn inline_commands(
    root: &Path,
    map: &Map<String, Value>,
    out: &mut Resolved,
) -> Result<(), String> {
    for (name, spec) in map {
        let Value::Object(spec) = spec else {
            let reason = "it is not an object";
            out.dropped
                .push(Dropped::new(format!("command \"{name}\""), reason));
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
            let reason = "it needs one of source or content";
            out.dropped
                .push(Dropped::new(format!("command \"{name}\""), reason));
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

/// Hooks merge across sources. With a plugin.json, an event in the strict entry's inline hooks
/// replaces the groups of that event from the plugin.
pub fn resolve_hooks(
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
                let reason = "with a plugin.json, entry hooks must be inline";
                out.dropped
                    .push(Dropped::new("marketplace entry hooks path", reason));
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
