use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};

use crate::catalog::{Catalog, Entry};
use crate::config::Config;
use crate::env::{Dirs, Env, Scope};
use crate::plugin::{self, Resolved};
use crate::source::{PluginSource, contained, ensure_inside};
use crate::stage::{Target, remove_path, stage_commands, stage_skills};
use crate::state::{PluginRecord, State, plugin_id, split_id};
use crate::{RELOAD, agents, bin, git, hooks, market, mcp, options, styles};

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

struct Staged {
    record: PluginRecord,
    counts: Vec<String>,
    notes: Vec<String>,
}

fn stage_all(
    env: &Env,
    scope: Scope,
    mp_scope: Scope,
    resolved: &Resolved,
    id: &str,
    mp: &str,
) -> Result<Staged> {
    let dirs = env.dirs(scope);
    let data_dir = dirs.plugin_data(mp, &resolved.name)?;
    std::fs::create_dir_all(&data_dir)?;
    let opts = options::schema(&resolved.user_config);
    let stored = options::load(&dirs.plugin_options(mp, &resolved.name)?)?;
    let target = Target {
        dirs: &dirs,
        plugin: &resolved.name,
        id: id.to_owned(),
        root: resolved.root.clone(),
        data_dir: data_dir.clone(),
        project_dir: (scope == Scope::Project).then(|| env.cwd.clone()),
        content_options: options::vars(&opts, &stored, false),
        config_options: options::vars(&opts, &stored, true),
    };
    let bin_names = resolved.bin.as_deref().map(bin::names).unwrap_or_default();
    let mut record = PluginRecord {
        plugin: resolved.name.clone(),
        marketplace: mp.to_owned(),
        marketplace_scope: mp_scope,
        version: resolved.version.clone(),
        description: resolved.description.clone(),
        enabled: true,
        plugin_root: resolved.root.clone(),
        files: Vec::new(),
        hooks: false,
        mcp_keys: Vec::new(),
        unsupported: resolved.unsupported.clone(),
        bin: resolved.bin.clone().filter(|_| !bin_names.is_empty()),
        bin_names,
        user_config: resolved.user_config.clone(),
        lsp: resolved
            .lsp
            .iter()
            .map(|(name, spec)| {
                let command = spec.get("command").and_then(|c| c.as_str()).unwrap_or("?");
                format!("{name}: {command}")
            })
            .collect(),
        default_agent: resolved.default_agent.clone(),
    };
    let mut notes = resolved.notes.clone();
    let missing = options::missing(&opts, &stored);
    if !missing.is_empty() {
        notes.push(format!(
            "Needs user config: {}. Set it with /claude-plugins config {id} KEY=VALUE",
            missing.join(", ")
        ));
    }
    if record.bin.is_some() && cfg!(windows) {
        notes.push("bin/ is not added to PATH on Windows".into());
    }
    let mut run = |record: &mut PluginRecord| -> Result<Vec<String>> {
        let commands = stage_commands(
            &target,
            &resolved.commands,
            &resolved.inline_commands,
            &mut record.files,
        )?;
        let skills = stage_skills(&target, &resolved.skills, &mut record.files)?;
        let agents = agents::stage_agents(&target, &resolved.agents, &mut record.files)?;
        let styles = styles::stage_styles(&target, &resolved.output_styles, &mut record.files)?;
        let tag = hooks::Tag {
            plugin_id: id,
            root: &resolved.root,
            data: &data_dir,
            vars: target.vars(None),
            arg_vars: target.config_vars(),
            env: options::env(&opts, &stored),
        };
        let groups = hooks::tagged_groups(&resolved.hooks, &tag);
        let hook_count: usize = groups
            .values()
            .filter_map(|g| g.as_array())
            .map(Vec::len)
            .sum();
        record.hooks = hook_count > 0;
        hooks::write(&dirs.hooks_file(), id, groups)?;
        let vars = target.config_vars();
        let (keys, mcp_notes) =
            mcp::write(&dirs.mcp_file(), id, &resolved.name, &resolved.mcp, &vars)?;
        notes.extend(mcp_notes);
        record.mcp_keys = keys;
        let mut counts = vec![
            format!("{commands} command(s)"),
            format!("{skills} skill(s)"),
            format!("{agents} agent(s)"),
            format!("{hook_count} hook group(s)"),
            format!("{} MCP server(s)", record.mcp_keys.len()),
        ];
        let extra = [
            (styles, "output style(s)"),
            (record.bin_names.len(), "bin command(s)"),
            (opts.len(), "user config option(s)"),
        ];
        for (count, what) in extra.into_iter().filter(|(count, _)| *count > 0) {
            counts.push(format!("{count} {what}"));
        }
        Ok(counts)
    };
    match run(&mut record) {
        Ok(counts) => Ok(Staged {
            record,
            counts,
            notes,
        }),
        Err(err) => {
            unstage(&record, &dirs, id)?;
            Err(err)
        }
    }
}

/// Installs or reinstalls `plugin@marketplace` in `scope`, with its missing dependencies.
pub fn install(env: &Env, id: &str, scope: Scope) -> Result<String> {
    install_with(env, id, scope, &mut Vec::new())
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
    let resolved =
        plugin::resolve(entry, &root).map_err(|e| anyhow!("{plugin} is unavailable: {e}"))?;
    let dependencies = install_dependencies(env, &resolved, mp, scope, seen);
    let mut state = State::load(&dirs)?;
    if let Some(old) = state.plugins.remove(id) {
        unstage(&old, &dirs, id)?;
        state.save(&dirs)?;
    }
    let staged = stage_all(env, scope, located.scope, &resolved, id, mp)?;
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
    if !staged.record.unsupported.is_empty() {
        text.push_str(&format!(
            " Not supported in pi: {}.",
            staged.record.unsupported.join(", ")
        ));
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
