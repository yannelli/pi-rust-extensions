use anyhow::Result;

use crate::config::Config;
use crate::env::Env;
use crate::state::State;
use crate::{RELOAD, install, market};

/// Updates every visible marketplace, reinstalls enabled plugins, then adds config
/// entries (marketplaces and enabled plugins) that the state does not have yet.
pub fn update_all(env: &Env) -> Result<String> {
    let mut lines = Vec::new();
    for located in market::all(env)? {
        match market::update(env, &located) {
            Ok(more) => lines.extend(more),
            Err(err) => lines.push(format!("{err:#}")),
        }
    }
    lines.extend(apply_config(env)?);
    if lines.is_empty() {
        return Ok("No marketplaces configured.".into());
    }
    lines.push(RELOAD.into());
    Ok(lines.join("\n"))
}

fn apply_config(env: &Env) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    for scope in env.visible_scopes() {
        let dirs = env.dirs(scope);
        let config = Config::load(&dirs)?;
        for (name, source, _) in config.marketplaces() {
            if State::load(&dirs)?.marketplaces.contains_key(&name) {
                continue;
            }
            match market::add(env, &source, scope) {
                Ok(text) => lines.push(text),
                Err(err) => lines.push(format!("Cannot add marketplace {name}: {err:#}")),
            }
        }
        for (id, enabled) in config.plugins() {
            if !enabled || State::load(&dirs)?.plugins.contains_key(&id) {
                continue;
            }
            match install::install(env, &id, scope) {
                Ok(text) => lines.push(text.replace(&format!(" {RELOAD}"), "")),
                Err(err) => lines.push(format!("Cannot install {id}: {err:#}")),
            }
        }
    }
    Ok(lines)
}

/// Updates the marketplaces with `autoUpdate: true`. Returns the lines of the ones it updated.
pub fn auto_update(env: &Env) -> Vec<String> {
    let mut lines = Vec::new();
    let Ok(all) = market::all(env) else {
        return lines;
    };
    for located in all {
        let auto = Config::load(&env.dirs(located.scope)).is_ok_and(|config| {
            config
                .marketplaces()
                .iter()
                .any(|(name, _, auto)| *auto && *name == located.record.name)
        });
        if !auto || located.record.git_url.is_none() {
            continue;
        }
        match market::update(env, &located) {
            Ok(more) => lines.extend(more),
            Err(err) => pi_ext_sdk::warn!("auto-update of {}: {err:#}", located.record.name),
        }
    }
    lines
}
