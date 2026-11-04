//! Stages the components of one resolved plugin into a scope.

use anyhow::Result;

use crate::env::{Env, Scope};
use crate::fields::{self, FrontDrops};
use crate::install::unstage;
use crate::plugin::Resolved;
use crate::settings::{ArgIndex, Settings};
use crate::stage::{Target, Vars, stage_commands, stage_skills};
use crate::state::{BinDir, PluginRecord};
use crate::{agents, bin, hooks, mcp, options, styles};

pub struct Staged {
    pub record: PluginRecord,
    pub counts: Vec<String>,
    pub notes: Vec<String>,
}

/// Where the plugin comes from and goes to.
pub struct Plan<'a> {
    pub scope: Scope,
    pub mp_scope: Scope,
    pub id: &'a str,
    pub mp: &'a str,
}

fn arguments_text(settings: &Settings) -> String {
    let (index, from) = &settings.arg_index;
    let mode = match index {
        ArgIndex::Auto => "auto",
        ArgIndex::Zero => "zero",
        ArgIndex::One => "one",
    };
    format!("{mode} ({from})")
}

fn new_record(plan: &Plan, resolved: &Resolved, settings: &Settings) -> PluginRecord {
    let bins: Vec<BinDir> = resolved
        .bins
        .iter()
        .map(|dir| BinDir {
            dir: dir.clone(),
            names: bin::names(dir),
        })
        .filter(|b| !b.names.is_empty())
        .collect();
    PluginRecord {
        plugin: resolved.name.clone(),
        marketplace: plan.mp.to_owned(),
        marketplace_scope: plan.mp_scope,
        version: resolved.version.clone(),
        description: resolved.description.clone(),
        enabled: true,
        plugin_root: resolved.root.clone(),
        files: Vec::new(),
        hooks: false,
        mcp_keys: Vec::new(),
        unsupported: resolved.unsupported.clone(),
        bins,
        env_file: None,
        dropped: resolved.dropped.clone(),
        strict: resolved.strict.clone(),
        arguments: arguments_text(settings),
        agent_models: Vec::new(),
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
    }
}

fn joined(parts: &[&Vars]) -> Vars {
    parts.iter().flat_map(|v| v.iter().cloned()).collect()
}

pub fn stage_all(
    env: &Env,
    plan: &Plan,
    resolved: &Resolved,
    settings: &Settings,
) -> Result<Staged> {
    let (scope, id) = (plan.scope, plan.id);
    let dirs = env.dirs(scope);
    let data_dir = dirs.plugin_data(plan.mp, &resolved.name)?;
    std::fs::create_dir_all(&data_dir)?;
    let opts = options::schema(&resolved.user_config);
    let stored = options::load(&dirs.plugin_options(plan.mp, &resolved.name)?)?;
    let open_env = options::env_vars(&opts, &stored, false);
    let all_env = options::env_vars(&opts, &stored, true);
    let extra = &settings.env;
    let target = Target {
        dirs: &dirs,
        plugin: &resolved.name,
        id: id.to_owned(),
        root: resolved.root.clone(),
        data_dir: data_dir.clone(),
        project_dir: (scope == Scope::Project).then(|| env.cwd.clone()),
        content_extra: joined(&[&options::vars(&opts, &stored, false), &open_env, extra]),
        config_extra: joined(&[&options::vars(&opts, &stored, true), &all_env, extra]),
        env_extra: joined(&[&all_env, extra]),
        arg_index: settings.arg_index.0,
        rules: settings.rules.clone(),
    };
    let mut record = new_record(plan, resolved, settings);
    let mut notes = Vec::new();
    let missing = options::missing(&opts, &stored);
    if !missing.is_empty() {
        notes.push(format!(
            "Needs user config: {}. Set it with /claude-plugins config {id} KEY=VALUE",
            missing.join(", ")
        ));
    }
    if !record.bins.is_empty() && cfg!(windows) {
        let reason = "pi adds plugin bin dirs to PATH on Unix only";
        record.dropped.push(fields::Dropped::new("bin/", reason));
    }
    let mut run = |record: &mut PluginRecord| -> Result<Vec<String>> {
        let mut drops = FrontDrops::new();
        let files = &mut record.files;
        let inline = &resolved.inline_commands;
        let commands = stage_commands(&target, &resolved.commands, inline, files, &mut drops)?;
        let skills = stage_skills(&target, &resolved.skills, files, &mut drops)?;
        let mut models = Vec::new();
        let agents =
            agents::stage_agents(&target, &resolved.agents, files, &mut drops, &mut models)?;
        let styles = styles::stage_styles(&target, &resolved.output_styles, files, &mut drops)?;
        drops.finish(&mut record.dropped);
        record.agent_models = models;
        if !record.bins.is_empty() {
            let file = dirs
                .env_files()
                .join(plan.mp)
                .join(format!("{}.sh", resolved.name));
            let script = bin::env_script(&target.process_env());
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&file, script)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))?;
            }
            record.files.push(file.clone());
            record.env_file = Some(file);
        }
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
        let (vars, process) = (target.config_vars(), target.process_env());
        let mcp_file = dirs.mcp_file();
        let (keys, mcp_notes) = mcp::write(
            &mcp_file,
            id,
            &resolved.name,
            &resolved.mcp,
            &vars,
            &process,
        )?;
        notes.extend(mcp_notes);
        record.mcp_keys = keys;
        let mut counts = vec![
            format!("{commands} command(s)"),
            format!("{skills} skill(s)"),
            format!("{agents} agent(s)"),
            format!("{hook_count} hook group(s)"),
            format!("{} MCP server(s)", record.mcp_keys.len()),
        ];
        let bin_count: usize = record.bins.iter().map(|b| b.names.len()).sum();
        let extra = [
            (styles, "output style(s)"),
            (bin_count, "bin command(s)"),
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
