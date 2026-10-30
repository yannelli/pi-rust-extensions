use anyhow::{Result, bail};

use crate::env::{Env, Scope};
use crate::listing;
use crate::{import, install, market, reconcile};

pub const HELP: &str = "\
/claude-plugins                      open the plugin browser
/claude-plugins marketplace add <git URL | owner/repo | path> [--scope user|project]
/claude-plugins marketplace list
/claude-plugins marketplace update [<marketplace>]
/claude-plugins marketplace remove <marketplace> [--scope user|project]
/claude-plugins marketplace autoupdate|noautoupdate <marketplace>
/claude-plugins list [<marketplace>] [--installed] [--available] [--unavailable]
/claude-plugins info <plugin>@<marketplace>
/claude-plugins install <plugin>@<marketplace> [--scope user|project]
/claude-plugins update [<plugin>@<marketplace>]
/claude-plugins uninstall <plugin>@<marketplace> [--scope user|project]
/claude-plugins enable|disable <plugin>@<marketplace>
/claude-plugins import [--scope user|project]
Changes apply to pi after /reload.";

#[derive(Debug, Default, PartialEq)]
pub struct Args {
    pub words: Vec<String>,
    pub scope: Option<Scope>,
    pub installed: bool,
    pub available: bool,
    pub unavailable: bool,
}

pub fn parse(text: &str) -> Result<Args> {
    let mut args = Args::default();
    let mut tokens = text.split_whitespace();
    while let Some(token) = tokens.next() {
        match token {
            "--scope" => match tokens.next() {
                Some(value) => args.scope = Some(Scope::parse(value)?),
                None => bail!("--scope needs user or project"),
            },
            "--installed" => args.installed = true,
            "--available" => args.available = true,
            "--unavailable" => args.unavailable = true,
            other => match other.strip_prefix("--scope=") {
                Some(value) => args.scope = Some(Scope::parse(value)?),
                None if other.starts_with("--") => bail!("unknown option {other}"),
                None => args.words.push(other.to_owned()),
            },
        }
    }
    Ok(args)
}

fn need<'a>(words: &'a [String], index: usize, what: &str) -> Result<&'a str> {
    match words.get(index) {
        Some(word) => Ok(word),
        None => bail!("missing {what}. Try /claude-plugins help"),
    }
}

pub fn list_text(env: &Env, args: &Args) -> Result<String> {
    let name = args.words.get(1).map(String::as_str);
    let all = listing::plugins(env, name, args.scope)?;
    if all.is_empty() {
        return Ok(match name {
            Some(name) => format!("Marketplace \"{name}\" not found."),
            None => "No marketplaces configured.".into(),
        });
    }
    let (lines, _) =
        listing::render_plugins(&all, args.installed, args.available, args.unavailable);
    Ok(lines.join("\n"))
}

pub fn info(env: &Env, id: &str) -> Result<String> {
    let (plugin, mp) = crate::state::split_id(id)?;
    let all = listing::plugins(env, Some(mp), None)?;
    let row = all
        .iter()
        .flat_map(|m| &m.plugins)
        .find(|p| p.name == plugin);
    let Some(row) = row else {
        bail!("{id} not found");
    };
    let mut lines = vec![format!("{id} [{}]", row.scope), listing::plugin_line(row)];
    lines.extend(row.description.clone());
    if let Ok((_, record)) = install::find(env, id, None) {
        lines.push(format!("Root: {}", record.plugin_root.display()));
        lines.push(format!("Files: {}", record.files.len()));
        if !record.mcp_keys.is_empty() {
            lines.push(format!("MCP servers: {}", record.mcp_keys.join(", ")));
        }
        if record.hooks {
            lines.push("Hooks: yes".into());
        }
    }
    Ok(lines.join("\n"))
}

fn marketplace_cmd(env: &Env, args: &Args) -> Result<String> {
    let words = &args.words;
    match need(words, 1, "marketplace subcommand")? {
        "add" => market::add(
            env,
            need(words, 2, "source")?,
            args.scope.unwrap_or(Scope::User),
        ),
        "list" | "ls" => {
            let rows = listing::marketplaces(env, args.scope)?;
            if rows.is_empty() {
                return Ok("No marketplaces configured.".into());
            }
            Ok(rows
                .iter()
                .map(listing::marketplace_line)
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "update" => match words.get(2) {
            Some(name) => {
                let located = market::find(env, name, args.scope)?;
                let mut lines = market::update(env, &located)?;
                lines.push(crate::RELOAD.into());
                Ok(lines.join("\n"))
            }
            None => reconcile::update_all(env),
        },
        "remove" | "rm" => market::remove(env, need(words, 2, "marketplace")?, args.scope),
        "autoupdate" => market::set_auto_update(env, need(words, 2, "marketplace")?, true),
        "noautoupdate" => market::set_auto_update(env, need(words, 2, "marketplace")?, false),
        other => bail!("unknown marketplace subcommand \"{other}\". Try /claude-plugins help"),
    }
}

/// Runs one text subcommand and returns the text to show.
pub fn run(env: &Env, text: &str) -> Result<String> {
    let args = parse(text)?;
    let words = &args.words;
    let Some(first) = words.first() else {
        return list_text(env, &args);
    };
    match first.as_str() {
        "help" => Ok(HELP.into()),
        "marketplace" | "mp" => marketplace_cmd(env, &args),
        "list" | "browse" => list_text(env, &args),
        "info" => info(env, need(words, 1, "plugin")?),
        "install" => install::install(
            env,
            need(words, 1, "plugin")?,
            args.scope.unwrap_or(Scope::User),
        ),
        "reinstall" => {
            let id = need(words, 1, "plugin")?;
            let scope = match args.scope {
                Some(scope) => scope,
                None => install::find(env, id, None)?.0,
            };
            install::install(env, id, scope)
        }
        "update" => match words.get(1) {
            Some(id) => install::update_one(env, id, args.scope),
            None => reconcile::update_all(env),
        },
        "uninstall" | "remove" => install::uninstall(env, need(words, 1, "plugin")?, args.scope),
        "enable" => install::set_enabled(env, need(words, 1, "plugin")?, true, args.scope),
        "disable" => install::set_enabled(env, need(words, 1, "plugin")?, false, args.scope),
        "import" => import::import(env, args.scope),
        other => bail!("unknown subcommand \"{other}\". Try /claude-plugins help"),
    }
}
