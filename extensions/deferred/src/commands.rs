//! `/deferred`: status, reload, reset, and rule edits.

use pi_ext_sdk::SettingsScope;
use pi_ext_sdk::prelude::*;

use crate::config::{self, Kind, Mode, NAMESPACE};
use crate::{Shared, fmt, load_config, lock};

pub(crate) const DESCRIPTION: &str = "Show what is deferred, or edit rules: [status|reload|reset|set <kind> <pattern> <mode>|unset <kind> <pattern>] [--project]";

const USAGE: &str = "Usage: /deferred [status|reload|reset], /deferred set <tools|skills|context|sections> <pattern> \
    <on|names|deferred|off> [--project], or /deferred unset <kind> <pattern> [--project].";

pub(crate) async fn run(state: Shared, args: String, ctx: CommandCtx) -> Result<()> {
    let words: Vec<&str> = args.split_whitespace().collect();
    let host = ctx.host();
    let text = match words.first().copied() {
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
    let project = words.contains(&"--project");
    let rest: Vec<&str> = words[1..]
        .iter()
        .copied()
        .filter(|w| *w != "--project")
        .collect();
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
    let host = ctx.host();
    let (path, scope) = match project {
        true => (
            host.project_dir().join("settings.json"),
            SettingsScope::Project,
        ),
        false => (
            host.agent_dir().join("settings.json"),
            SettingsScope::Global,
        ),
    };
    let block = config::read_file(&path).and_then(|v| v.get(NAMESPACE).cloned());
    let block = config::edit_rule(block, kind, pattern, mode);
    host.set_settings(NAMESPACE, block, scope)
        .await
        .map_err(|err| format!("Could not write {}: {}", path.display(), err.message()))?;
    reload(state, ctx).await;
    let change = match mode {
        Some(mode) => format!("Set {} `{pattern}` to {}", kind.key(), mode.as_str()),
        None => format!("Removed the {} rule `{pattern}`", kind.key()),
    };
    Ok(format!(
        "{change} in {}. It applies from the next prompt.",
        path.display()
    ))
}
