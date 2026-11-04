use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};

use crate::catalog::{Catalog, Entry};
use crate::config::Config;
use crate::env::{Dirs, Env, Scope};
use crate::plugin::{self, Resolved};
use crate::source::{PluginSource, contained, ensure_inside};
use crate::stage::remove_path;
use crate::state::{PluginRecord, State, plugin_id, split_id};
use crate::{RELOAD, apply, fields, git, hooks, market, mcp, options, settings};

/// Marketplaces a plugin of `scope` can come from: the same scope, then user.
fn marketplace_scopes(scope: Scope) -> Vec<Scope> {
    match scope {
        Scope::User => vec![Scope::User],
        Scope::Project => vec![Scope::Project, Scope::User],
    }
}

/// The plugin directory: inside the marketplace, or a fresh clone.
pub fn materialize(entry: &Entry, catalog: &Catalog, dirs: &Dirs) -> Result<PathBuf> {
    match &entry.source {
        PluginSource::Path(rel) => catalog.local_root(rel).map_err(|e| anyhow!(e)),
        PluginSource::Git {
            url,
            git_ref,
            sha,
            subdir,
        } => {
            let clone = dirs.plugin_clone(&catalog.name, &entry.name)?;
            let incoming = clone.with_file_name(format!(".{}.incoming", entry.name));
            remove_path(&incoming)?;
            if let Err(err) = git::clone(url, &incoming, git_ref.as_deref(), sha.as_deref()) {
                remove_path(&incoming)?;
                return Err(err);
            }
            remove_path(&clone)?;
            std::fs::rename(&incoming, &clone)?;
            let root = match subdir {
                Some(sub) => contained(&clone, sub).map_err(|e| anyhow!(e))?,
                None => clone,
            };
            if !root.is_dir() {
                bail!("plugin directory {} does not exist", root.display());
            }
            Ok(root)
        }
        PluginSource::Unsupported(reason) => bail!("{} is unavailable: {reason}", entry.name),
    }
}

/// Removes the staged files, hook groups, and MCP servers of a record.
pub fn unstage(record: &PluginRecord, dirs: &Dirs, id: &str) -> Result<()> {
    let bases = [
        dirs.prompts(),
        dirs.skills(),
        dirs.agents(),
        dirs.output_styles(),
        dirs.env_files(),
    ];
    for file in &record.files {
        if bases.iter().any(|base| ensure_inside(base, file).is_ok()) {
            remove_path(file)?;
        } else {
            pi_ext_sdk::warn!("not removing {}: outside the staging dirs", file.display());
        }
    }
    hooks::remove(&dirs.hooks_file(), id)?;
    mcp::remove(&dirs.mcp_file(), id)
}

/// Installs or reinstalls `plugin@marketplace` in `scope`, with its missing dependencies.
pub fn install(env: &Env, id: &str, scope: Scope) -> Result<String> {
    install_with(env, id, scope, &mut Vec::new())
}

/// `install <source>`: adds the source as a marketplace (or finds the one added from it) and
/// installs its plugin when it has exactly one.
pub fn install_source(env: &Env, raw: &str, scope: Scope) -> Result<String> {
    let mp_scopes = marketplace_scopes(scope);
    let existing = mp_scopes.iter().find_map(|&s| {
        let state = State::load(&env.dirs(s)).ok()?;
        let source = crate::source::parse_marketplace(raw, &env.cwd, &env.home).ok()?;
        let wanted = match source {
            crate::source::MarketplaceSource::Git { url } => url,
            crate::source::MarketplaceSource::Local { root, manifest } => {
                match manifest.is_file() {
                    true => manifest.display().to_string(),
                    false => root.display().to_string(),
                }
            }
        };
        state.marketplaces.into_values().find(|r| {
            r.git_url.as_deref() == Some(wanted.as_str())
                || r.source == wanted
                || r.source == raw.trim()
        })
    });
    let (name, mut lines) = match existing {
        Some(record) => (record.name, Vec::new()),
        None => {
            let (name, text) = market::add_named(env, raw, scope)?;
            (name, vec![text])
        }
    };
    let located = market::find_in(env, &name, &mp_scopes)?;
    let catalog = located.catalog()?;
    match catalog.plugins.as_slice() {
        [one] => lines.push(install(env, &plugin_id(&one.name, &name), scope)?),
        plugins => lines.push(format!(
            "Marketplace {name} has {} plugins; install one with /claude-plugins install <plugin>@{name}.",
            plugins.len()
        )),
    }
    Ok(lines.join("\n"))
}

fn install_dependencies(
    env: &Env,
    resolved: &Resolved,
    mp: &str,
    scope: Scope,
    seen: &mut Vec<String>,
) -> Vec<String> {
    let mut lines = Vec::new();
    for dep in &resolved.dependencies {
        let dep_id = match dep.contains('@') {
            true => dep.clone(),
            false => plugin_id(dep, mp),
        };
        if seen.contains(&dep_id) || find(env, &dep_id, None).is_ok() {
            continue;
        }
        match install_with(env, &dep_id, scope, seen) {
            Ok(text) => lines.push(format!(
                "Dependency: {}",
                text.replace(&format!(" {RELOAD}"), "")
            )),
            Err(err) => lines.push(format!("Cannot install dependency {dep_id}: {err:#}.")),
        }
    }
    lines
}

fn install_with(env: &Env, id: &str, scope: Scope, seen: &mut Vec<String>) -> Result<String> {
    env.require(scope)?;
    seen.push(id.to_owned());
    let (plugin, mp) = split_id(id)?;
    let located = market::find_in(env, mp, &marketplace_scopes(scope))?;
    let catalog = located.catalog()?;
    let Some(entry) = catalog.entry(plugin) else {
        bail!("plugin \"{plugin}\" is not in marketplace {mp}");
    };
    let dirs = env.dirs(scope);
    let root = materialize(entry, &catalog, &dirs)?;
    let settings = settings::load(env, scope, id)?;
    let resolved = plugin::resolve_with(entry, &root, &settings)
        .map_err(|e| anyhow!("{plugin} is unavailable: {e}"))?;
    let dependencies = install_dependencies(env, &resolved, mp, scope, seen);
    let mut state = State::load(&dirs)?;
    if let Some(old) = state.plugins.remove(id) {
        unstage(&old, &dirs, id)?;
        state.save(&dirs)?;
    }
    let plan = apply::Plan {
        scope,
        mp_scope: located.scope,
        id,
        mp,
    };
    let staged = apply::stage_all(env, &plan, &resolved, &settings)?;
    state.plugins.insert(id.to_owned(), staged.record.clone());
    state.save(&dirs)?;
    let mut config = Config::load(&dirs)?;
    config.set_plugin(id, true)?;
    config.save()?;
    let version = staged
        .record
        .version
        .as_deref()
        .map(|v| format!(" {v}"))
        .unwrap_or_default();
    let mut text = format!(
        "Installed {id}{version} [{scope}]: {}.",
        staged.counts.join(", ")
    );
    let dropped = fields::summary(&staged.record.dropped);
    if !dropped.is_empty() {
        text.push_str(&format!(" Not applied in pi: {}.", dropped.join("; ")));
    }
    for note in &staged.notes {
        text.push_str(&format!(" {note}."));
    }
    for line in dependencies {
        text.push_str(&format!("\n{line}"));
    }
    text.push_str(&format!(" {RELOAD}"));
    Ok(text)
}

/// The options of an installed plugin and their stored values.
pub fn user_options(
    env: &Env,
    id: &str,
    scope: Option<Scope>,
) -> Result<(
    Scope,
    Vec<options::Opt>,
    serde_json::Map<String, serde_json::Value>,
)> {
    let (scope, record) = find(env, id, scope)?;
    let file = env
        .dirs(scope)
        .plugin_options(&record.marketplace, &record.plugin)?;
    Ok((
        scope,
        options::schema(&record.user_config),
        options::load(&file)?,
    ))
}

/// Sets (`Some`) or clears (`None`) user config values, then reinstalls an enabled plugin.
pub fn configure(
    env: &Env,
    id: &str,
    scope: Option<Scope>,
    updates: &[(String, Option<String>)],
) -> Result<String> {
    let (scope, record) = find(env, id, scope)?;
    let opts = options::schema(&record.user_config);
    let file = env
        .dirs(scope)
        .plugin_options(&record.marketplace, &record.plugin)?;
    let mut stored = options::load(&file)?;
    if updates.is_empty() {
        if opts.is_empty() {
            return Ok(format!("{id} has no user config."));
        }
        let mut lines = vec![format!("User config of {id} [{scope}]:")];
        lines.extend(options::lines(&opts, &stored));
        lines.push(format!(
            "Set with /claude-plugins config {id} KEY=VALUE; KEY= clears a value."
        ));
        return Ok(lines.join("\n"));
    }
    env.require(scope)?;
    for (key, value) in updates {
        let Some(opt) = opts.iter().find(|o| &o.key == key) else {
            bail!("{id} has no user config option \"{key}\"");
        };
        match value {
            Some(text) => stored.insert(key.clone(), options::parse(opt, text)?),
            None => stored.shift_remove(key),
        };
    }
    options::save(&file, &stored)?;
    if !record.enabled {
        return Ok(format!("Saved the user config of {id} [{scope}]."));
    }
    install(env, id, scope).map(|text| text.replacen("Installed", "Configured", 1))
}

/// The scope and record of an installed plugin.
pub fn find(env: &Env, id: &str, scope: Option<Scope>) -> Result<(Scope, PluginRecord)> {
    let scopes = match scope {
        Some(scope) => {
            env.require(scope)?;
            vec![scope]
        }
        None => env.visible_scopes(),
    };
    for scope in scopes {
        if let Some(record) = State::load(&env.dirs(scope))?.plugins.get(id) {
            return Ok((scope, record.clone()));
        }
    }
    bail!("{id} is not installed")
}

pub fn uninstall(env: &Env, id: &str, scope: Option<Scope>) -> Result<String> {
    let (scope, record) = find(env, id, scope)?;
    env.require(scope)?;
    let dirs = env.dirs(scope);
    unstage(&record, &dirs, id)?;
    remove_path(&dirs.plugin_clone(&record.marketplace, &record.plugin)?)?;
    remove_path(&dirs.plugin_data(&record.marketplace, &record.plugin)?)?;
    remove_path(&dirs.plugin_options(&record.marketplace, &record.plugin)?)?;
    let mut state = State::load(&dirs)?;
    state.plugins.remove(id);
    state.save(&dirs)?;
    let mut config = Config::load(&dirs)?;
    config.remove_plugin(id)?;
    config.save()?;
    Ok(format!("Uninstalled {id} [{scope}]. {RELOAD}"))
}

pub fn set_enabled(env: &Env, id: &str, enabled: bool, scope: Option<Scope>) -> Result<String> {
    let (scope, record) = find(env, id, scope)?;
    env.require(scope)?;
    if record.enabled == enabled {
        let word = if enabled { "enabled" } else { "disabled" };
        return Ok(format!("{id} is already {word}."));
    }
    if enabled {
        return install(env, id, scope).map(|text| text.replacen("Installed", "Enabled", 1));
    }
    let dirs = env.dirs(scope);
    unstage(&record, &dirs, id)?;
    let mut state = State::load(&dirs)?;
    if let Some(entry) = state.plugins.get_mut(id) {
        entry.enabled = false;
        entry.files.clear();
        entry.hooks = false;
        entry.mcp_keys.clear();
    }
    state.save(&dirs)?;
    let mut config = Config::load(&dirs)?;
    config.set_plugin(id, false)?;
    config.save()?;
    Ok(format!("Disabled {id} [{scope}]. {RELOAD}"))
}

/// Updates one plugin's marketplace and reinstalls it.
pub fn update_one(env: &Env, id: &str, scope: Option<Scope>) -> Result<String> {
    let (scope, record) = find(env, id, scope)?;
    if !record.enabled {
        bail!("{id} is disabled; enable it first");
    }
    let located = market::find_in(env, &record.marketplace, &[record.marketplace_scope])?;
    if located.record.git_url.is_some() {
        git::update(&located.record.root)?;
    }
    install(env, id, scope).map(|text| text.replacen("Installed", "Updated", 1))
}
