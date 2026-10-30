use std::path::PathBuf;

use anyhow::Result;
use serde_json::{Map, Value};

use crate::env::{Env, Scope};
use crate::source::{OFFICIAL, OFFICIAL_REPO};
use crate::state::{State, split_id};
use crate::{RELOAD, install, jsonfile, market};

fn settings_files(env: &Env, scope: Scope) -> Vec<PathBuf> {
    let dir = match scope {
        Scope::User => env.claude_dir.clone(),
        Scope::Project => env.cwd.join(".claude"),
    };
    vec![dir.join("settings.json"), dir.join("settings.local.json")]
}

/// The source text of an `extraKnownMarketplaces` entry.
pub fn known_source(entry: &Value) -> Option<String> {
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_owned);
    if let Some(source) = entry.get("source") {
        if let Some(raw) = source.as_str() {
            return Some(raw.to_owned());
        }
        return text(source, "repo")
            .or_else(|| text(source, "url"))
            .or_else(|| text(source, "path"))
            .or_else(|| text(source, "directory"));
    }
    text(entry, "directory").or_else(|| entry.get("github").and_then(|g| text(g, "repo")))
}

struct Imported {
    marketplaces: Map<String, Value>,
    plugins: Vec<(String, bool)>,
}

fn read(env: &Env, scope: Scope) -> Result<Imported> {
    let mut marketplaces = Map::new();
    let mut plugins: Vec<(String, bool)> = Vec::new();
    for file in settings_files(env, scope) {
        let data = jsonfile::read_object(&file)?;
        if let Some(Value::Object(known)) = data.get("extraKnownMarketplaces") {
            marketplaces.extend(known.clone());
        }
        if let Some(Value::Object(enabled)) = data.get("enabledPlugins") {
            for (id, on) in enabled {
                let on = on.as_bool().unwrap_or(false);
                plugins.retain(|(existing, _)| existing != id);
                plugins.push((id.clone(), on));
            }
        }
    }
    Ok(Imported {
        marketplaces,
        plugins,
    })
}

fn ensure_marketplace(
    env: &Env,
    scope: Scope,
    name: &str,
    imported: &Imported,
    lines: &mut Vec<String>,
) -> bool {
    let scopes: &[Scope] = match scope {
        Scope::User => &[Scope::User],
        Scope::Project => &[Scope::Project, Scope::User],
    };
    if market::find_in(env, name, scopes).is_ok() {
        return true;
    }
    let source = imported
        .marketplaces
        .get(name)
        .and_then(known_source)
        .or_else(|| (name == OFFICIAL).then(|| OFFICIAL_REPO.to_owned()));
    let Some(source) = source else {
        lines.push(format!(
            "Skipped marketplace {name}: no source in the Claude settings."
        ));
        return false;
    };
    match market::add(env, &source, scope) {
        Ok(text) => lines.push(text),
        Err(err) => {
            lines.push(format!("Cannot add marketplace {name}: {err:#}"));
            return false;
        }
    }
    let found = market::find_in(env, name, scopes).is_ok();
    if !found {
        lines.push(format!("Marketplace from {source} is not named {name}."));
    }
    found
}

/// Imports `extraKnownMarketplaces` and `enabledPlugins` from the Claude Code settings.
pub fn import(env: &Env, scope: Option<Scope>) -> Result<String> {
    let scopes = match scope {
        Some(scope) => {
            env.require(scope)?;
            vec![scope]
        }
        None => vec![Scope::User, Scope::Project],
    };
    let mut lines = Vec::new();
    for scope in scopes {
        let imported = read(env, scope)?;
        if imported.marketplaces.is_empty() && imported.plugins.is_empty() {
            continue;
        }
        if scope == Scope::Project && !env.trusted {
            lines.push("Skipped the project Claude settings: the project is not trusted.".into());
            continue;
        }
        let names: Vec<String> = imported.marketplaces.keys().cloned().collect();
        for name in names {
            ensure_marketplace(env, scope, &name, &imported, &mut lines);
        }
        let installed = State::load(&env.dirs(scope))?.plugins;
        for (id, enabled) in &imported.plugins {
            if !enabled || installed.contains_key(id) {
                continue;
            }
            let Ok((_, mp)) = split_id(id) else {
                lines.push(format!("Skipped {id}: not <plugin>@<marketplace>."));
                continue;
            };
            if !ensure_marketplace(env, scope, mp, &imported, &mut lines) {
                continue;
            }
            match install::install(env, id, scope) {
                Ok(text) => lines.push(text.replace(&format!(" {RELOAD}"), "")),
                Err(err) => lines.push(format!("Cannot install {id}: {err:#}")),
            }
        }
    }
    if lines.is_empty() {
        return Ok("Nothing to import from the Claude Code settings.".into());
    }
    lines.push(RELOAD.into());
    Ok(lines.join("\n"))
}
