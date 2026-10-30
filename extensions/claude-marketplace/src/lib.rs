use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pi_ext_sdk::CompletionItem;
use pi_ext_sdk::prelude::*;
use serde::Deserialize;

pub mod agents;
pub mod catalog;
pub mod cli;
pub mod complete;
pub mod config;
pub mod env;
pub mod frontmatter;
pub mod git;
pub mod hooks;
pub mod import;
pub mod install;
mod jsonfile;
pub mod listing;
pub mod market;
pub mod mcp;
pub mod plugin;
pub mod reconcile;
pub mod source;
pub mod stage;
pub mod state;
mod tui;

pub use env::{Env, Paths, Scope};

pub const RELOAD: &str = "Run /reload to apply the changes in pi.";
const COMMANDS: [&str; 2] = ["claude-plugins", "claude:plugin"];

pub fn main() -> ExitCode {
    extension(Paths::default()).run()
}

/// Runs a sync operation on a blocking thread.
pub async fn blocking<T, F>(env: Env, op: F) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: FnOnce(Env) -> anyhow::Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(move || op(env)).await?
}

fn trusted(snapshot: &Snapshot) -> bool {
    let value = snapshot.extra.get("isProjectTrusted");
    value.and_then(Value::as_bool).unwrap_or(false)
}

fn env_of(paths: &Paths, host: &Host, snapshot: Option<&Snapshot>) -> Env {
    let cwd = snapshot
        .filter(|s| !s.cwd.is_empty())
        .map(Snapshot::cwd_path)
        .unwrap_or_else(|| host.cwd());
    Env::new(paths, host.agent_dir(), cwd, snapshot.is_some_and(trusted))
}

#[derive(Deserialize, Default)]
struct PluginListArgs {
    marketplace: Option<String>,
    scope: Option<String>,
    installed: Option<bool>,
    available: Option<bool>,
    unavailable: Option<bool>,
}

fn list_marketplaces(env: &Env) -> anyhow::Result<ToolResult> {
    let rows = listing::marketplaces(env, None)?;
    if rows.is_empty() {
        return Ok(
            ToolResult::text("No marketplaces configured.").details(json!({ "marketplaces": [] }))
        );
    }
    let text: Vec<String> = rows.iter().map(listing::marketplace_line).collect();
    Ok(ToolResult::text(text.join("\n")).details(json!({ "marketplaces": rows })))
}

fn list_plugins(env: &Env, args: PluginListArgs) -> anyhow::Result<ToolResult> {
    let scope = args.scope.as_deref().map(Scope::parse).transpose()?;
    let all = listing::plugins(env, args.marketplace.as_deref(), scope)?;
    if all.is_empty() {
        let text = match &args.marketplace {
            Some(name) => format!("Marketplace \"{name}\" not found."),
            None => "No marketplaces configured.".into(),
        };
        return Ok(ToolResult::text(text).details(json!({ "plugins": [] })));
    }
    let flag = |value: Option<bool>| value.unwrap_or(false);
    let (lines, rows) = listing::render_plugins(
        &all,
        flag(args.installed),
        flag(args.available),
        flag(args.unavailable),
    );
    Ok(ToolResult::text(lines.join("\n")).details(json!({ "plugins": rows })))
}

fn tool_specs() -> (ToolSpec, ToolSpec) {
    let list = ToolSpec::new(
        "pi_claude_marketplace_list",
        "List configured Claude plugin marketplaces.",
    )
    .label("Claude Marketplace List")
    .prompt_snippet(
        "Use pi_claude_marketplace_list to inspect configured Claude plugin marketplaces.",
    )
    .parameters(json!({ "type": "object", "properties": {} }));
    let flag = |text: &str| json!({ "type": "boolean", "description": text });
    let plugins = ToolSpec::new(
        "pi_claude_marketplace_plugin_list",
        "List plugins in a Claude marketplace, showing compatibility and install status.",
    )
    .label("Marketplace Plugin List")
    .prompt_snippet("Use pi_claude_marketplace_plugin_list to inspect plugins in a marketplace.")
    .parameters(json!({
        "type": "object",
        "properties": {
            "marketplace": { "type": "string", "description": "Marketplace name to list plugins for." },
            "scope": { "type": "string", "enum": ["user", "project"],
                "description": "Scope to look in: \"user\" or \"project\". Default: both scopes." },
            "installed": flag("Include plugins installed in state.json."),
            "available": flag("Include manifest-declared plugins that are not installed but are installable."),
            "unavailable": flag("Include manifest-declared plugins that are not installable on this system.")
        }
    }));
    (list, plugins)
}

fn into_tool(result: anyhow::Result<ToolResult>) -> ToolResult {
    result.unwrap_or_else(|err| ToolResult::error(format!("{err:#}")))
}

fn discover(env: &Env) -> Value {
    let mut skills: Vec<PathBuf> = Vec::new();
    let mut prompts: Vec<PathBuf> = Vec::new();
    for scope in [Scope::User, Scope::Project] {
        if !env.visible_scopes().contains(&scope) {
            continue;
        }
        let dirs = env.dirs(scope);
        skills.extend(Some(dirs.skills()).filter(|d| d.is_dir()));
        prompts.extend(Some(dirs.prompts()).filter(|d| d.is_dir()));
    }
    json!({ "skillPaths": skills, "promptPaths": prompts })
}

fn untrusted_project_files(env: &Env) -> bool {
    let dirs = env.dirs(Scope::Project);
    !env.trusted && (dirs.config_file().is_file() || dirs.state_file().is_file())
}

/// The extension with injected paths; [`main`] uses [`Paths::default`].
pub fn extension(paths: Paths) -> Extension {
    let paths = Arc::new(paths);
    let warned = Arc::new(AtomicBool::new(false));
    let (list_spec, plugins_spec) = tool_specs();
    let p = paths.clone();
    let mut ext = Extension::new(env::NAME).tool(list_spec, move |_: Value, ctx: ToolCtx| {
        let env = env_of(&p, ctx.host(), Some(ctx.snapshot()));
        async move {
            Ok(into_tool(
                blocking(env, |env| list_marketplaces(&env)).await,
            ))
        }
    });
    let p = paths.clone();
    ext = ext.tool(plugins_spec, move |args: PluginListArgs, ctx: ToolCtx| {
        let env = env_of(&p, ctx.host(), Some(ctx.snapshot()));
        async move {
            Ok(into_tool(
                blocking(env, move |env| list_plugins(&env, args)).await,
            ))
        }
    });
    for name in COMMANDS {
        let p = paths.clone();
        let description = "Claude Code plugin marketplaces: browse, install, update, remove";
        ext = ext.command(name, description, move |args: String, ctx: CommandCtx| {
            let host = ctx.host().clone();
            let env = env_of(&p, &host, Some(ctx.snapshot()));
            async move {
                if args.trim().is_empty() && host.has_ui() {
                    return tui::browse(host, env).await;
                }
                match blocking(env, move |env| cli::run(&env, &args)).await {
                    Ok(text) => host.notify_info(text).await,
                    Err(err) => host.notify_error(format!("{err:#}")).await,
                }
            }
        });
        let p = paths.clone();
        ext = ext.completions(name, move |prefix: String, host: Host| {
            let env = env_of(&p, &host, host.snapshot().as_deref());
            async move {
                let values =
                    blocking(env, move |env| Ok(complete::complete(&env, &prefix))).await?;
                Ok(Some(values.into_iter().map(CompletionItem::new).collect()))
            }
        });
    }
    let p = paths.clone();
    ext = ext.on("resources_discover", move |ev: EventCtx| {
        let mut env = env_of(&p, ev.host(), Some(ev.snapshot()));
        if let Some(cwd) = ev.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()) {
            env.cwd = PathBuf::from(cwd);
        }
        let host = ev.host().clone();
        let warned = warned.clone();
        async move {
            if untrusted_project_files(&env) && !warned.swap(true, Ordering::SeqCst) {
                tokio::spawn(async move {
                    let text = "claude-marketplace: the project is not trusted, so its Claude plugins are not loaded.";
                    let _ = host.notify_warning(text).await;
                });
            }
            Ok(EventReply::result(discover(&env)))
        }
    });
    let p = paths.clone();
    ext.on_start(move |host: Host| {
        let env = env_of(&p, &host, host.snapshot().as_deref());
        async move {
            let lines = blocking(env, |env| Ok(reconcile::auto_update(&env))).await?;
            if !lines.is_empty() {
                host.notify_info(format!("{}\n{RELOAD}", lines.join("\n")))
                    .await?;
            }
            Ok(())
        }
    })
}
