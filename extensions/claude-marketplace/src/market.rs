use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::catalog::Catalog;
use crate::config::Config;
use crate::env::{Env, Scope};
use crate::install;
use crate::source::{MarketplaceSource, parse_marketplace};
use crate::stage::remove_path;
use crate::state::{MarketplaceRecord, State};
use crate::{RELOAD, git};

#[derive(Debug, Clone)]
pub struct Located {
    pub scope: Scope,
    pub record: MarketplaceRecord,
}

impl Located {
    pub fn catalog(&self) -> Result<Catalog> {
        Catalog::read(&self.record.manifest, &self.record.root)
    }
}

/// Finds a marketplace in `scopes`, first match wins.
pub fn find_in(env: &Env, name: &str, scopes: &[Scope]) -> Result<Located> {
    for &scope in scopes {
        let state = State::load(&env.dirs(scope))?;
        if let Some(record) = state.marketplaces.get(name) {
            return Ok(Located {
                scope,
                record: record.clone(),
            });
        }
    }
    bail!("marketplace \"{name}\" is not added; use /claude-plugins marketplace add <source>")
}

pub fn find(env: &Env, name: &str, scope: Option<Scope>) -> Result<Located> {
    match scope {
        Some(scope) => {
            env.require(scope)?;
            find_in(env, name, &[scope])
        }
        None => find_in(env, name, &env.visible_scopes()),
    }
}

pub fn all(env: &Env) -> Result<Vec<Located>> {
    let mut out = Vec::new();
    for scope in env.visible_scopes() {
        let state = State::load(&env.dirs(scope))?;
        out.extend(
            state
                .marketplaces
                .into_values()
                .map(|record| Located { scope, record }),
        );
    }
    Ok(out)
}

fn incoming_dir(sources: &Path) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    sources.join(format!(".incoming-{}-{nanos}", std::process::id()))
}

pub fn add(env: &Env, raw: &str, scope: Scope) -> Result<String> {
    env.require(scope)?;
    let dirs = env.dirs(scope);
    let mut state = State::load(&dirs)?;
    let parsed = parse_marketplace(raw, &env.cwd, &env.home)?;
    let record = match parsed {
        MarketplaceSource::Git { url } => {
            let tmp = incoming_dir(&dirs.sources());
            git::clone(&url, &tmp, None, None)?;
            let manifest = tmp.join(".claude-plugin/marketplace.json");
            let catalog = Catalog::read(&manifest, &tmp);
            let name = match catalog {
                Ok(catalog) if !state.marketplaces.contains_key(&catalog.name) => catalog.name,
                Ok(catalog) => {
                    remove_path(&tmp)?;
                    bail!(
                        "marketplace \"{}\" is already added in {scope} scope",
                        catalog.name
                    );
                }
                Err(err) => {
                    remove_path(&tmp)?;
                    return Err(err);
                }
            };
            let dest = dirs.sources().join(&name);
            remove_path(&dest)?;
            std::fs::rename(&tmp, &dest)
                .with_context(|| format!("cannot move clone to {}", dest.display()))?;
            MarketplaceRecord {
                name,
                source: raw.trim().to_owned(),
                git_url: Some(url),
                manifest: dest.join(".claude-plugin/marketplace.json"),
                root: dest,
            }
        }
        MarketplaceSource::Local { root, manifest } => {
            let catalog = Catalog::read(&manifest, &root)?;
            if state.marketplaces.contains_key(&catalog.name) {
                bail!(
                    "marketplace \"{}\" is already added in {scope} scope",
                    catalog.name
                );
            }
            MarketplaceRecord {
                name: catalog.name,
                source: manifest.display().to_string(),
                git_url: None,
                root,
                manifest,
            }
        }
    };
    let count = Catalog::read(&record.manifest, &record.root)?.plugins.len();
    let name = record.name.clone();
    let mut config = Config::load(&dirs)?;
    config.set_marketplace(&name, &record.source)?;
    state.marketplaces.insert(name.clone(), record);
    state.save(&dirs)?;
    config.save()?;
    Ok(format!(
        "Added marketplace {name} ({count} plugin(s)) in {scope} scope. Install with /claude-plugins install <plugin>@{name}."
    ))
}

/// Scopes and ids of installed plugins that come from `located`.
pub fn dependents(env: &Env, located: &Located) -> Result<Vec<(Scope, String)>> {
    let mut out = Vec::new();
    for scope in env.visible_scopes() {
        let state = State::load(&env.dirs(scope))?;
        for (id, record) in state.plugins {
            if record.marketplace == located.record.name
                && record.marketplace_scope == located.scope
            {
                out.push((scope, id));
            }
        }
    }
    Ok(out)
}

/// Fetches the marketplace again and reinstalls its enabled plugins.
pub fn update(env: &Env, located: &Located) -> Result<Vec<String>> {
    let name = &located.record.name;
    if located.record.git_url.is_some() {
        git::update(&located.record.root).with_context(|| format!("cannot update {name}"))?;
    }
    let catalog = located.catalog()?;
    let mut lines = vec![format!(
        "Updated marketplace {name} [{}]: {} plugin(s).",
        located.scope,
        catalog.plugins.len()
    )];
    for (scope, id) in dependents(env, located)? {
        let state = State::load(&env.dirs(scope))?;
        if !state.plugins.get(&id).is_some_and(|r| r.enabled) {
            continue;
        }
        match install::install(env, &id, scope) {
            Ok(text) => lines.push(text.replace(&format!(" {RELOAD}"), "")),
            Err(err) => lines.push(format!("Cannot update {id}: {err:#}")),
        }
    }
    Ok(lines)
}

pub fn remove(env: &Env, name: &str, scope: Option<Scope>) -> Result<String> {
    let located = find(env, name, scope)?;
    env.require(located.scope)?;
    let mut lines = Vec::new();
    for (plugin_scope, id) in dependents(env, &located)? {
        install::uninstall(env, &id, Some(plugin_scope))?;
        lines.push(format!("Uninstalled {id} [{plugin_scope}]."));
    }
    let dirs = env.dirs(located.scope);
    if located.record.git_url.is_some() && located.record.root.starts_with(dirs.sources()) {
        remove_path(&located.record.root)?;
    }
    remove_path(&dirs.state_dir.join("plugin-clones").join(name))?;
    let mut state = State::load(&dirs)?;
    state.marketplaces.remove(name);
    state.save(&dirs)?;
    let mut config = Config::load(&dirs)?;
    config.remove_marketplace(name)?;
    config.save()?;
    lines.push(format!(
        "Removed marketplace {name} [{}]. {RELOAD}",
        located.scope
    ));
    Ok(lines.join("\n"))
}

pub fn set_auto_update(env: &Env, name: &str, auto: bool) -> Result<String> {
    let located = find(env, name, None)?;
    let dirs = env.dirs(located.scope);
    let mut config = Config::load(&dirs)?;
    if !config.set_auto_update(name, auto)? {
        config.set_marketplace(name, &located.record.source)?;
        config.set_auto_update(name, auto)?;
    }
    config.save()?;
    let word = if auto { "on" } else { "off" };
    Ok(format!("Auto-update for {name} is {word}."))
}
