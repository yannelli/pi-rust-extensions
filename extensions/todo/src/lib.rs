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

const DESCRIPTION: &str = "Manage a task list for tracking multi-step progress. Actions: create (new task, or several independent tasks at once via tasks[]), update (change status/fields/dependencies), list (all tasks, optionally filtered by status), get (single task details), delete (tombstone), clear (reset all), move (reorder a task). Phases: add_phase, update_phase, set_goal, delete_phase group tasks into ordered phases with an optional goal. Status: pending → in_progress → completed, plus deleted tombstone. Use this to plan and track multi-step work like research, design, and implementation.";

const SNIPPET: &str = "Manage a task list to track multi-step progress";

const GUIDELINES: &[&str] = &[
    "Use `todo` for complex work with 3+ steps, when the user gives you a list of tasks, or immediately after receiving new instructions to capture requirements. Skip it for single trivial tasks and purely conversational requests.",
    "When starting a task from the todo list, mark it in_progress BEFORE beginning work. Mark it completed IMMEDIATELY when done — never batch completions. Exactly one task in_progress at a time.",
    "Never mark a task completed if tests are failing, the implementation is partial, or you hit unresolved errors — keep it in_progress and create a new task for the blocker instead.",
    "Task status is a 4-state machine: pending → in_progress → completed, plus deleted as a tombstone. Pass activeForm (present-continuous label, e.g. 'researching existing tool') when marking in_progress.",
    "To change a task's status, call update with the task id and the target status, e.g. {\"action\":\"update\",\"id\":3,\"status\":\"completed\"} or {\"action\":\"update\",\"id\":3,\"status\":\"in_progress\",\"activeForm\":\"writing tests\"}. status is the field that changes the task; an update without a mutable field (status or another) is rejected.",
    "Use blockedBy to express dependencies (A is blocked by B). On create, pass blockedBy as the initial set. On update, use addBlockedBy / removeBlockedBy (additive merge — do not resend the full array). Cycles are rejected.",
    "To create several independent tasks at once, pass create with tasks: [{subject}, ...]. Do not also pass subject. tasks[] cannot blockedBy each other (ids do not exist until the call commits); create the prerequisite first, then the dependent with blockedBy. Dependency chains stay single-create.",
    "list hides tombstoned (deleted) tasks by default; pass includeDeleted:true to see them. Pass status to filter by a single status.",
    "Subject must be short and imperative (e.g. 'Research existing tool'); description is for long-form detail. activeForm is a present-continuous label shown while in_progress.",
    "For larger plans, group tasks into ordered phases: add_phase {title, goal?} returns a phase id P<n>; pass phase:<n> on create (or on update to move a task). Mark a phase in_progress or completed with update_phase {phase, status}; set_goal {phase, goal} sets or clears its goal.",
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
