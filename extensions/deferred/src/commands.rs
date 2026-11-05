//! `/deferred`: status, reload, reset, and rule edits.

use pi_ext_sdk::SettingsScope;
use pi_ext_sdk::prelude::*;

use crate::config::{self, Kind, Mode, NAMESPACE};
use crate::ui::rows::Scope;
use crate::{Shared, fmt, load_config, lock};

pub(crate) const DESCRIPTION: &str = "Browse and change what is deferred, or: [status|reload|reset|set <kind> <pattern> <mode>|unset <kind> <pattern>] [--project|--repo <owner/name>]";

const USAGE: &str = "Usage: /deferred (the panel), /deferred [status|reload|reset], /deferred set <tools|skills|context|sections> <pattern> \
    <on|names|deferred|user|off> [--project|--repo <owner/name>], or /deferred unset <kind> <pattern> [--project|--repo <owner/name>]. \
    `user` is for skills only; `--repo` without a name uses the cwd's repo.";

pub(crate) async fn run(state: Shared, args: String, ctx: CommandCtx) -> Result<()> {
    let words: Vec<&str> = args.split_whitespace().collect();
    let host = ctx.host();
    let text = match words.first().copied() {
        None if host.has_ui() => return crate::ui::run::open(state, &ctx).await,
        None | Some("status") => status(&state),
        Some("reload") => {
            reload(&state, &ctx).await;
            format!("Reloaded the config.\n{}", status(&state))
        }
        Some("reset") => {
            let count = std::mem::take(&mut lock(&state).loaded).len();
            format!(
                "Forgot {count} loaded tool(s); the rules apply to them again from the next prompt."
            )
        }
        Some("set" | "unset") => match edit(&state, &ctx, &words).await {
            Ok(text) => text,
            Err(text) => return host.notify_error(text).await,
        },
        Some(other) => {
            return host
                .notify_warning(format!("Unknown subcommand `{other}`. {USAGE}"))
                .await;
        }
    };
    host.notify_info(text).await
}

fn status(state: &Shared) -> String {
    let session = lock(state);
    let loaded: Vec<String> = session.loaded.iter().cloned().collect();
    fmt::status(
        &session.config,
        &session.sources,
        session.plan.as_ref(),
        &loaded,
    )
}

async fn reload(state: &Shared, ctx: &CommandCtx) {
    let (config, sources) = load_config(ctx.host(), ctx.snapshot()).await;
    let mut session = lock(state);
    session.config = config;
    session.sources = sources;
}

/// `set` or `unset` one rule in the global or project `settings.json`.
async fn edit(
    state: &Shared,
    ctx: &CommandCtx,
    words: &[&str],
) -> std::result::Result<String, String> {
    let unset = words[0] == "unset";
    let current = lock(state).config.repo.clone();
    let (project, repo, rest) = flags(&words[1..], current)?;
    let (kind, pattern, mode) = match (unset, rest.as_slice()) {
        (false, [kind, pattern, mode]) => (*kind, *pattern, Some(*mode)),
        (true, [kind, pattern]) => (*kind, *pattern, None),
        _ => return Err(USAGE.to_owned()),
    };
    let kind = Kind::parse(kind).ok_or_else(|| format!("Unknown kind `{kind}`. {USAGE}"))?;
    let mode = match mode {
        Some(text) => {
            Some(Mode::parse(text).ok_or_else(|| format!("Unknown mode `{text}`. {USAGE}"))?)
        }
        None => None,
    };
    if mode.is_some_and(|m| !m.fits(kind)) {
        return Err(format!("`user` is for skills only. {USAGE}"));
    }
    let scope = match (project, repo) {
        (true, Some(_)) => return Err(format!("Use `--project` or `--repo`, not both. {USAGE}")),
        (true, None) => Scope::Project,
        (false, Some(repo)) => Scope::Repo(repo),
        (false, None) => Scope::Global,
    };
    write_rule(state, ctx, kind, pattern, mode, &scope).await
}

/// `--project`, `--repo [owner/name]`, and the other words.
fn flags<'a>(
    words: &[&'a str],
    current: Option<String>,
) -> std::result::Result<(bool, Option<String>, Vec<&'a str>), String> {
    let (mut project, mut repo, mut rest) = (false, None, Vec::new());
    let mut iter = words.iter().copied().peekable();
    while let Some(word) = iter.next() {
        match word {
            "--project" => project = true,
            "--repo" => {
                let named = iter.next_if(|next| next.contains('/') && !next.starts_with("--"));
                let name = named.map(str::to_owned).or_else(|| current.clone());
                repo = Some(name.ok_or_else(|| {
                    "`--repo` needs <owner/name>: the cwd has no `origin` remote.".to_owned()
                })?);
            }
            _ => rest.push(word),
        }
    }
    Ok((project, repo, rest))
}

/// Sets (`Some`) or removes (`None`) the rule `kind.pattern` in the `scope` settings file,
/// then reloads the config.
pub(crate) async fn write_rule(
    state: &Shared,
    ctx: &CommandCtx,
    kind: Kind,
    pattern: &str,
    mode: Option<Mode>,
    scope: &Scope,
) -> std::result::Result<String, String> {
    let host = ctx.host();
    let (path, target) = match scope {
        Scope::Project => (host.project_file("settings.json"), SettingsScope::Project),
        Scope::Global | Scope::Repo(_) => (
            host.agent_dir().join("settings.json"),
            SettingsScope::Global,
        ),
    };
    let block = config::read_file(&path).and_then(|v| v.get(NAMESPACE).cloned());
    let block = match scope {
        Scope::Repo(repo) => config::edit_repo_rule(block, repo, kind, pattern, mode),
        _ => config::edit_rule(block, kind, pattern, mode),
    };
    host.set_settings(NAMESPACE, block, target)
        .await
        .map_err(|err| format!("Could not write {}: {}", path.display(), err.message()))?;
    reload(state, ctx).await;
    let change = match mode {
        Some(mode) => format!("Set {} `{pattern}` to {}", kind.key(), mode.as_str()),
        None => format!("Removed the {} rule `{pattern}`", kind.key()),
    };
    let repo = match scope {
        Scope::Repo(repo) => format!(" for repo {repo}"),
        _ => String::new(),
    };
    Ok(format!(
        "{change}{repo} in {}. It applies from the next prompt.",
        path.display()
    ))
}
