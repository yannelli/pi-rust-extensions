//! File edits behind the `/hooks` panel: disable, enable, remove, and add.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::ser::PrettyFormatter;
use serde_json::{Map, Value, json};
use toml_edit::{ArrayOfTables, DocumentMut, Item};

use crate::config::codex::table_to_json;
use crate::model::{HookDef, Scope, Source};

pub const PROJECT_UNTRUSTED: &str =
    "project is not trusted; trust it in pi before changing project hooks";

/// Events that `/hooks` offers when adding a hook.
pub const EVENTS: [&str; 16] = [
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PostToolBatch",
    "PermissionRequest",
    "UserPromptSubmit",
    "Stop",
    "StopFailure",
    "SubagentStart",
    "SubagentStop",
    "SessionStart",
    "SessionEnd",
    "PreCompact",
    "PostCompact",
    "PreModelSwitch",
    "PostModelSwitch",
];

pub fn check_scope(scope: Scope, project_trusted: bool) -> Result<()> {
    if scope == Scope::Project && !project_trusted {
        bail!(PROJECT_UNTRUSTED);
    }
    Ok(())
}

/// Adds or removes `def.hash` in the `disabled` list of the pi options file `file`.
pub fn set_disabled(file: &Path, def: &HookDef, disabled: bool, trusted: bool) -> Result<()> {
    check_scope(def.scope, trusted)?;
    if !disabled && !file.exists() {
        return Ok(());
    }
    let mut doc = JsonDoc::load(file)?;
    let root = doc.value.as_object_mut().expect("checked object");
    let mut list: Vec<String> = root
        .get("disabled")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let had = list.contains(&def.hash);
    if had == disabled {
        return Ok(());
    }
    if disabled {
        list.push(def.hash.clone());
    } else {
        list.retain(|h| h != &def.hash);
    }
    if list.is_empty() {
        root.remove("disabled");
    } else {
        root.insert("disabled".into(), json!(list));
    }
    doc.save(file)
}

/// Deletes the handler of `def` from its config file and drops emptied groups and events.
pub fn remove(def: &HookDef, trusted: bool) -> Result<()> {
    check_scope(def.scope, trusted)?;
    if def.file.extension().is_some_and(|e| e == "toml") {
        return remove_toml(def);
    }
    let mut doc = JsonDoc::load(&def.file)?;
    let events = doc
        .value
        .get_mut("hooks")
        .and_then(Value::as_object_mut)
        .context("no hooks object")?;
    let groups = events
        .get_mut(&def.event)
        .and_then(Value::as_array_mut)
        .context("event not found")?;
    let mut found = None;
    for (gi, group) in groups.iter().enumerate() {
        if group.get("matcher").and_then(Value::as_str) != def.matcher.as_deref() {
            continue;
        }
        let handlers = group.get("hooks").and_then(Value::as_array);
        if let Some(hi) = handlers.and_then(|h| h.iter().position(|h| *h == def.handler)) {
            found = Some((gi, hi));
            break;
        }
    }
    let (gi, hi) = found.with_context(|| format!("hook not found in {}", def.file.display()))?;
    let handlers = groups[gi]["hooks"].as_array_mut().expect("found above");
    handlers.remove(hi);
    if handlers.is_empty() {
        groups.remove(gi);
    }
    if groups.is_empty() {
        events.remove(&def.event);
    }
    doc.save(&def.file)
}

fn remove_toml(def: &HookDef) -> Result<()> {
    let text = std::fs::read_to_string(&def.file)?;
    let mut doc: DocumentMut = text.parse()?;
    let events = doc
        .get_mut("hooks")
        .and_then(Item::as_table_like_mut)
        .context("no hooks table")?;
    let groups = events
        .get_mut(&def.event)
        .and_then(Item::as_array_of_tables_mut)
        .context("event not found as [[hooks.<Event>]]")?;
    let found = find_toml(groups, def)
        .with_context(|| format!("hook not found in {}", def.file.display()))?;
    let (gi, hi) = found;
    let group = groups.get_mut(gi).expect("found above");
    let handlers = group["hooks"]
        .as_array_of_tables_mut()
        .expect("found above");
    handlers.remove(hi);
    if handlers.is_empty() {
        groups.remove(gi);
    }
    if groups.is_empty() {
        events.remove(&def.event);
    }
    std::fs::write(&def.file, doc.to_string())?;
    Ok(())
}

fn find_toml(groups: &ArrayOfTables, def: &HookDef) -> Option<(usize, usize)> {
    groups.iter().enumerate().find_map(|(gi, group)| {
        let matcher = group.get("matcher").and_then(Item::as_str);
        if matcher != def.matcher.as_deref() {
            return None;
        }
        let handlers = group.get("hooks")?.as_array_of_tables()?;
        let hi = handlers
            .iter()
            .position(|h| table_to_json(h) == def.handler)?;
        Some((gi, hi))
    })
}

/// Appends a command hook to the pi-native file `file` and returns it.
pub fn add(
    file: &Path,
    scope: Scope,
    trusted: bool,
    event: &str,
    matcher: Option<&str>,
    command: &str,
) -> Result<HookDef> {
    check_scope(scope, trusted)?;
    let matcher = matcher.map(str::trim).filter(|m| !m.is_empty());
    let command = command.trim();
    if command.is_empty() {
        bail!("the command is empty");
    }
    let mut doc = JsonDoc::load(file)?;
    let root = doc.value.as_object_mut().expect("checked object");
    let events = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context("`hooks` is not an object")?;
    let groups = events
        .entry(event)
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .with_context(|| format!("`hooks.{event}` is not an array"))?;
    let handler = json!({ "type": "command", "command": command });
    let mut group = Map::new();
    if let Some(m) = matcher {
        group.insert("matcher".into(), json!(m));
    }
    group.insert("hooks".into(), json!([handler.clone()]));
    groups.push(Value::Object(group));
    doc.save(file)?;
    let matcher = matcher.map(str::to_owned);
    Ok(HookDef::new(
        Source::Pi,
        scope,
        file.to_path_buf(),
        event,
        matcher,
        handler,
    ))
}

/// A JSON object file with the indent and trailing newline it had on disk.
struct JsonDoc {
    value: Value,
    indent: Option<String>,
    newline: bool,
}

impl JsonDoc {
    fn load(path: &Path) -> Result<Self> {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Ok(Self {
                value: Value::Object(Map::new()),
                indent: Some("  ".into()),
                newline: true,
            });
        };
        let value: Value = serde_json::from_str(&text)
            .with_context(|| format!("cannot parse {}", path.display()))?;
        if !value.is_object() {
            bail!("{} does not hold a JSON object", path.display());
        }
        let indent = text.lines().nth(1).map(|line| {
            let width = line.len() - line.trim_start().len();
            line[..width].to_owned()
        });
        let indent = match indent {
            Some(i) if i.is_empty() => Some("  ".into()),
            other => other,
        };
        let newline = text.ends_with('\n') || text.trim().is_empty();
        Ok(Self {
            value,
            indent,
            newline,
        })
    }

    fn save(&self, path: &Path) -> Result<()> {
        let mut out = Vec::new();
        match &self.indent {
            Some(indent) => {
                let formatter = PrettyFormatter::with_indent(indent.as_bytes());
                let mut ser = serde_json::Serializer::with_formatter(&mut out, formatter);
                self.value.serialize(&mut ser)?;
            }
            None => serde_json::to_writer(&mut out, &self.value)?,
        }
        if self.newline {
            out.push(b'\n');
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, out).with_context(|| format!("cannot write {}", path.display()))
    }
}

#[cfg(test)]
mod tests;
