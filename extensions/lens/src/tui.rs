use std::sync::Arc;

use pi_ext_sdk::prelude::*;
use tokio::sync::Mutex;

use crate::{config, state::State};

pub async fn manage(host: &Host, state: Arc<Mutex<State>>) -> Result<()> {
    loop {
        let actions = [
            "Status",
            "Enable or disable Lens",
            "Toggle context",
            "Toggle read guard",
            "Languages",
            "Tools",
            "Commands",
            "LSP servers",
            "Edit config",
        ];
        let Some(action) = host.select("Lens", &actions).await? else {
            return Ok(());
        };
        if action == "Status" {
            let text = {
                let state = state.lock().await;
                format!(
                    "Lens enabled={} context={} readGuard={} findings={}",
                    state.config.enabled,
                    state.config.context,
                    state.config.read_guard,
                    state.all_findings().len()
                )
            };
            host.notify_info(text).await?;
            continue;
        }
        let Some(scope) = host
            .select("Save Lens settings", &["Project", "Global"])
            .await?
        else {
            continue;
        };
        let paths = config::paths(&host.agent_dir(), &host.cwd());
        let path = &paths[usize::from(scope == "Project")];
        let mut value = config::read(path)?;
        if !value.is_object() {
            return Err(Error::msg("Lens config must be an object"));
        }
        let current = state.lock().await.config.clone();
        match action.as_str() {
            "Enable or disable Lens" => value["enabled"] = json!(!current.enabled),
            "Toggle context" => value["context"] = json!(!current.context),
            "Toggle read guard" => value["readGuard"] = json!(!current.read_guard),
            "Languages" => {
                let Some(language) = host
                    .select("Toggle language", &["rust", "javascript", "python", "go"])
                    .await?
                else {
                    continue;
                };
                *slot(&mut value, &["languages", &language, "enabled"])? =
                    json!(!current.language_enabled(&language));
            }
            "Tools" => {
                let mut names = vec![
                    "rustfmt",
                    "cargo-check",
                    "clippy",
                    "prettier",
                    "eslint",
                    "tsc",
                    "ruff-format",
                    "ruff",
                    "gofmt",
                    "go-vet",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>();
                names.extend(current.commands.iter().map(|item| item.name.clone()));
                names.extend(current.tools.keys().cloned());
                names.sort();
                names.dedup();
                let Some(name) = host.select("Toggle tool", &names).await? else {
                    continue;
                };
                *slot(&mut value, &["tools", &name, "enabled"])? =
                    json!(!current.tool_enabled(&name));
            }
            "Commands" | "LSP servers" => {
                let lsp = action == "LSP servers";
                let items = if lsp {
                    value["lsp"]["servers"].as_array()
                } else {
                    value["commands"].as_array()
                }
                .cloned()
                .unwrap_or_default();
                if let Some(items) = edit_items(host, items, lsp).await? {
                    let keys: &[&str] = if lsp {
                        &["lsp", "servers"]
                    } else {
                        &["commands"]
                    };
                    *slot(&mut value, keys)? = json!(items);
                } else {
                    continue;
                }
            }
            "Edit config" => {
                let prefill = serde_json::to_string_pretty(&value)?;
                let Some(text) = host.editor("Lens config JSON", Some(&prefill)).await? else {
                    continue;
                };
                value = serde_json::from_str(&text)?;
            }
            _ => continue,
        }
        config::save(path, &value)?;
        crate::refresh(host, &state).await?;
        host.notify_info(format!("Saved {}", path.display()))
            .await?;
    }
}

async fn edit_items(host: &Host, mut items: Vec<Value>, lsp: bool) -> Result<Option<Vec<Value>>> {
    let mut choices = vec!["Add".to_owned()];
    choices.extend(items.iter().enumerate().map(|(index, item)| {
        format!(
            "{}: {} [{}]",
            index + 1,
            item[if lsp { "command" } else { "name" }]
                .as_str()
                .unwrap_or("unnamed"),
            if item["enabled"] == false {
                "off"
            } else {
                "on"
            }
        )
    }));
    let Some(choice) = host.select("Configured items", &choices).await? else {
        return Ok(None);
    };
    let index = choices.iter().position(|item| *item == choice).unwrap_or(0);
    let template = if lsp {
        json!({"command":"", "args":[], "languages":["rust"], "enabled":true})
    } else {
        json!({"name":"", "language":"rust", "stage":"lint", "command":"", "args":["{path}"], "enabled":true})
    };
    if index == 0 {
        let text = serde_json::to_string_pretty(&template)?;
        if let Some(text) = host.editor("Add item JSON", Some(&text)).await? {
            items.push(serde_json::from_str(&text)?);
        } else {
            return Ok(None);
        }
    } else {
        let index = index - 1;
        let Some(action) = host
            .select("Item", &["Edit", "Enable or disable", "Delete"])
            .await?
        else {
            return Ok(None);
        };
        match action.as_str() {
            "Edit" => {
                let text = serde_json::to_string_pretty(&items[index])?;
                if let Some(text) = host.editor("Edit item JSON", Some(&text)).await? {
                    items[index] = serde_json::from_str(&text)?;
                } else {
                    return Ok(None);
                }
            }
            "Enable or disable" => {
                let enabled = items[index]["enabled"] == false;
                *slot(&mut items[index], &["enabled"])? = json!(enabled);
            }
            "Delete"
                if host
                    .confirm("Delete item?", "Remove this item from this config?")
                    .await? =>
            {
                items.remove(index);
            }
            _ => return Ok(None),
        }
    }
    Ok(Some(items))
}

/// The value at `keys`, after it turns null parents into objects. Other parent
/// types are an error, because `serde_json` panics when it indexes them.
pub(crate) fn slot<'a>(value: &'a mut Value, keys: &[&str]) -> Result<&'a mut Value> {
    let mut current = value;
    for key in keys {
        if current.is_null() {
            *current = json!({});
        }
        let map = current.as_object_mut().ok_or_else(|| {
            Error::msg(format!(
                "Lens config: the parent of \"{key}\" is not an object"
            ))
        })?;
        current = map.entry((*key).to_owned()).or_insert(Value::Null);
    }
    Ok(current)
}
