//! `todo`: rpiv-todo's task list with phases, subagent scopes, a widget, and `/todos`.

pub mod config;
pub mod format;
mod graph;
pub mod model;
pub mod panel;
pub mod panel_view;
pub mod params;
mod phases;
pub mod reducer;
pub mod replay;
pub mod scopes;
pub mod store;
mod update;
pub mod widget;

use std::process::ExitCode;

use pi_ext_sdk::prelude::*;

use crate::config::{COLLAPSE_KEY_OFF, Config};
use crate::replay::TOOL_NAME;
use crate::scopes::{SCOPE_CHANNEL, ScopeEvent};
use crate::store::Store;
use crate::widget::WIDGET_KEY;

pub const SCOPE_ENV: &str = "PI_TODO_SCOPE";

const DESCRIPTION: &str =
    "Track tasks and phases for complex work with 3+ steps or a user-provided task list.";

const SNIPPET: &str = "Track multi-step work";

const GUIDELINES: &[&str] = &[
    "Capture new requirements in todo. Advance pending → in_progress before work → completed immediately when done; deleted is a tombstone. Keep exactly one in_progress, with activeForm. Leave partial, failing, or blocked work in_progress and add a blocker task.",
    "Updates need a mutable field. Dependencies must be acyclic; create prerequisites before dependents. Use phases to group larger plans.",
];

fn tool_spec(config: &Config) -> ToolSpec {
    let mut spec = ToolSpec::new(TOOL_NAME, DESCRIPTION)
        .label("Todo")
        .parameters(params::schema())
        .prompt_snippet(
            config
                .prompt_snippet
                .clone()
                .unwrap_or_else(|| SNIPPET.into()),
        );
    let guidelines = config
        .prompt_guidelines
        .clone()
        .unwrap_or_else(|| GUIDELINES.iter().map(|g| (*g).to_owned()).collect());
    for guideline in guidelines {
        spec = spec.guideline(guideline);
    }
    spec
}

async fn command(store: Store, args: String, host: Host) -> Result<()> {
    let text_only = args.trim() == "list" || !host.has_ui();
    if !text_only {
        return panel::run(store, host).await;
    }
    let text = format::grouped(&store.lock().state);
    let text = text.unwrap_or_else(|| "No todos yet. Ask the agent to add some!".into());
    host.notify_info(text).await
}

async fn reload(store: Store, host: Host) -> Result<()> {
    let collapse_key = store.lock().config.collapse_key.clone();
    let mut config = config::load(&host.agent_dir(), &host.cwd());
    config.collapse_key = collapse_key;
    store.lock().config = config;
    store.replay(&host).await
}

async fn listen(store: Store, host: Host) -> Result<()> {
    let mut subscription = host.subscribe(SCOPE_CHANNEL).await?;
    while let Some(message) = subscription.recv_as::<ScopeEvent>().await {
        match message {
            Ok(event) => store.scope_event(&host, event).await?,
            Err(err) => pi_ext_sdk::warn!("ignoring a bad {SCOPE_CHANNEL} message: {err}"),
        }
    }
    Ok(())
}

/// The extension; `scope` is the subagent run id from `PI_TODO_SCOPE`, `None` in the main session.
pub fn extension(config: Config, scope: Option<String>) -> Extension {
    let store = Store::new(config.clone(), scope);
    let mut ext = Extension::new("todo");
    let s = store.clone();
    ext = ext.tool(tool_spec(&config), move |args: Value, ctx: ToolCtx| {
        let store = s.clone();
        async move { store.execute(ctx.host(), args).await }
    });
    let s = store.clone();
    ext = ext.command(
        "todos",
        "View and manage todos, phases, goals, and subagent scopes",
        move |args, ctx: CommandCtx| command(s.clone(), args, ctx.host().clone()),
    );
    if config.collapse_key != COLLAPSE_KEY_OFF {
        let s = store.clone();
        ext = ext.shortcut(
            &config.collapse_key,
            "Collapse or expand the todo overlay",
            move |ctx: CommandCtx| {
                let store = s.clone();
                async move { store.toggle_collapse(ctx.host()).await }
            },
        );
    }
    for event in ["session_start", "session_tree", "session_compact"] {
        let s = store.clone();
        ext = ext.on(event, move |ev: EventCtx| {
            reload(s.clone(), ev.host().clone())
        });
    }
    let s = store.clone();
    ext = ext.on("agent_start", move |ev: EventCtx| {
        let store = s.clone();
        async move { store.agent_start(ev.host()).await }
    });
    let s = store.clone();
    ext = ext.on_surface(move |event: SurfaceEvent, host: Host| {
        let store = s.clone();
        async move {
            match event {
                SurfaceEvent::Resize { .. } if event.widget_key() == Some(WIDGET_KEY) => {
                    store.refresh(&host).await
                }
                _ => Ok(()),
            }
        }
    });
    if store.scope().is_none() {
        ext = ext.on_start(move |host: Host| listen(store.clone(), host));
    }
    ext
}

pub fn main() -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_default();
    let config = config::load(&pi_ext_sdk::paths::agent_dir(), &cwd);
    let scope = std::env::var(SCOPE_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty());
    extension(config, scope).run()
}
