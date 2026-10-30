use std::path::Path;

use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::frontmatter::substitute;
use crate::jsonfile;

fn substitute_handler(handler: &mut Value, vars: &[(&str, String)]) {
    if let Some(Value::String(command)) = handler.get_mut("command") {
        *command = substitute(command, vars);
    }
    if let Some(Value::Array(args)) = handler.get_mut("args") {
        for arg in args.iter_mut() {
            if let Some(text) = arg.as_str() {
                *arg = json!(substitute(text, vars));
            }
        }
    }
}

/// Matcher groups for the pi-native hooks file, tagged with the plugin.
pub fn tagged_groups(
    configs: &[Map<String, Value>],
    plugin_id: &str,
    root: &Path,
    data: &Path,
    vars: &[(&str, String)],
) -> Map<String, Value> {
    let mut out: Map<String, Value> = Map::new();
    for config in configs {
        for (event, groups) in config {
            let list = out
                .entry(event.clone())
                .or_insert_with(|| Value::Array(Vec::new()));
            let Value::Array(list) = list else { continue };
            for group in groups.as_array().into_iter().flatten() {
                let Value::Object(group) = group else {
                    continue;
                };
                let mut group = group.clone();
                group.insert("plugin".into(), json!(plugin_id));
                group.insert("pluginRoot".into(), json!(root.display().to_string()));
                group.insert("pluginData".into(), json!(data.display().to_string()));
                if let Some(Value::Array(handlers)) = group.get_mut("hooks") {
                    handlers
                        .iter_mut()
                        .for_each(|handler| substitute_handler(handler, vars));
                }
                list.push(Value::Object(group));
            }
        }
    }
    out
}

fn strip_plugin(hooks: &mut Map<String, Value>, plugin_id: &str) -> bool {
    let mut removed = false;
    hooks.retain(|_, groups| {
        let Value::Array(list) = groups else {
            return true;
        };
        let before = list.len();
        list.retain(|group| group.get("plugin").and_then(Value::as_str) != Some(plugin_id));
        removed |= list.len() != before;
        !(list.is_empty() && before > 0)
    });
    removed
}

/// Replaces the groups of `plugin_id` in `file` with `groups`; other keys stay.
pub fn write(file: &Path, plugin_id: &str, groups: Map<String, Value>) -> Result<()> {
    let mut data = jsonfile::read_object(file)?;
    let hooks = jsonfile::object_entry(&mut data, "hooks")?;
    let removed = strip_plugin(hooks, plugin_id);
    let added = groups
        .values()
        .any(|g| g.as_array().is_some_and(|l| !l.is_empty()));
    for (event, new_groups) in groups {
        let Value::Array(new_groups) = new_groups else {
            continue;
        };
        if new_groups.is_empty() {
            continue;
        }
        let list = hooks
            .entry(event)
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(list) = list {
            list.extend(new_groups);
        }
    }
    if removed || added {
        jsonfile::write(file, &Value::Object(data))?;
    }
    Ok(())
}

pub fn remove(file: &Path, plugin_id: &str) -> Result<()> {
    write(file, plugin_id, Map::new())
}
