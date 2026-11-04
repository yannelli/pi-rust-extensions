use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, MutexGuard};

use pi_ext_sdk::event_types::ToolResultEvent;
use pi_ext_sdk::prelude::*;

pub mod blobs;
pub mod clean;
mod commands;
pub mod config;
pub mod files;
pub mod jsonfmt;
pub mod lines;
pub mod pipeline;
pub mod retrieve;
pub mod stack;
pub mod stats;
pub mod store;
pub mod toon;
pub mod ui;

use config::{Config, NAME};
use pipeline::GET_TOOL;
use stats::Stats;
use store::Store;

#[derive(Default)]
pub(crate) struct Session {
    pub(crate) config: Config,
    pub(crate) store: Option<Store>,
    pub(crate) stats: Stats,
    /// The trusted project file that overrides the global one.
    pub(crate) project: Option<PathBuf>,
}

pub(crate) type Shared = Arc<Mutex<Session>>;

pub(crate) fn lock(state: &Shared) -> MutexGuard<'_, Session> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

async fn project_trusted(host: &Host, snapshot: &Snapshot) -> bool {
    if snapshot.is_project_trusted {
        return true;
    }
    let answer = host.pi_call("ctx", "isProjectTrusted", Vec::new()).await;
    answer.ok().and_then(|v| v.as_bool()).unwrap_or(false)
}

/// The global file, then a trusted project file over it.
pub(crate) async fn reload(state: &Shared, host: &Host, snapshot: &Snapshot) {
    let global = files::read_file(&files::global_path(&host.agent_dir()));
    let project_path = files::project_path(&snapshot.cwd_path());
    let trusted = project_path.is_file() && project_trusted(host, snapshot).await;
    let project = trusted.then(|| files::read_file(&project_path)).flatten();
    let mut session = lock(state);
    session.project = project.is_some().then_some(project_path);
    session.config = files::load(global, project);
}

fn bind_store(session: &mut Session, host: &Host, snapshot: &Snapshot) -> Store {
    if let Some(store) = &session.store {
        return store.clone();
    }
    let id = snapshot
        .session_id
        .clone()
        .or_else(|| host.snapshot().and_then(|s| s.session_id.clone()));
    let store = Store::new(store::session_dir(&host.agent_dir(), id.as_deref()));
    session.store = Some(store.clone());
    store
}

async fn session_start(state: Shared, ev: EventCtx) -> Result<EventReply> {
    let host = ev.host();
    reload(&state, host, ev.snapshot()).await;
    let agent_dir = host.agent_dir();
    let mut session = lock(&state);
    session.store = None;
    let store = bind_store(&mut session, host, ev.snapshot());
    if session.stats.total.is_empty() {
        session.stats = Stats::load(&stats::path(&agent_dir));
    }
    drop(session);
    tokio::task::spawn_blocking(move || store::prune(&agent_dir, &store.dir));
    Ok(EventReply::none())
}

/// Stored results last for the session; a `/reload` keeps them.
async fn session_shutdown(state: Shared, ev: EventCtx) -> Result<EventReply> {
    let reason = ev.get("reason").and_then(Value::as_str).unwrap_or_default();
    let mut session = lock(&state);
    if reason != "reload" {
        if let Some(store) = session.store.take() {
            store.clear();
        }
    }
    let _ = session.stats.save(&stats::path(&ev.host().agent_dir()));
    Ok(EventReply::none())
}

fn text_size(content: &[Value]) -> usize {
    content
        .iter()
        .filter(|b| b["type"] == "text")
        .map(|b| b["text"].as_str().unwrap_or_default().chars().count())
        .sum()
}

async fn tool_result(state: Shared, ev: EventCtx) -> Result<EventReply> {
    let event: ToolResultEvent = ev.event_as()?;
    if event.tool_name == GET_TOOL {
        return Ok(EventReply::none());
    }
    let (settings, dry, context, store) = {
        let mut session = lock(&state);
        let settings = session.config.for_tool(&event.tool_name);
        let store = bind_store(&mut session, ev.host(), ev.snapshot());
        let config = &session.config;
        (settings, config.dry_run, config.context_chars, store)
    };
    let settings = match event.is_error {
        true => settings.for_error(),
        false => Some(settings),
    };
    let Some(settings) = settings.filter(|s| s.enabled) else {
        return Ok(EventReply::none());
    };
    let size = text_size(&event.content);
    let id = store.free_id(&store::result_id(&event.tool_call_id));
    let content = event.content.clone();
    let worker = store.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let outcome = pipeline::process(&content, &settings, &id, context)?;
        match (&outcome.stored, dry) {
            (Some(text), false) => match worker.save(&id, text) {
                Ok(()) => Some(outcome),
                Err(err) => {
                    pi_ext_sdk::warn!("tool-results: cannot store {id}: {err}");
                    None
                }
            },
            _ => Some(outcome),
        }
    })
    .await
    .map_err(|err| Error::msg(format!("tool-results worker failed: {err}")))?;
    let agent_dir = ev.host().agent_dir();
    let mut session = lock(&state);
    session
        .stats
        .record(&event.tool_name, outcome.as_ref(), size, dry);
    let _ = session.stats.save(&stats::path(&agent_dir));
    drop(session);
    match outcome {
        Some(outcome) if !dry => Ok(EventReply::patch(json!({ "content": outcome.content }))),
        _ => Ok(EventReply::none()),
    }
}

async fn get_tool(state: Shared, args: retrieve::Args, ctx: ToolCtx) -> Result<ToolResult> {
    let (store, config) = {
        let mut session = lock(&state);
        let store = bind_store(&mut session, ctx.host(), ctx.snapshot());
        (store, session.config.clone())
    };
    let Some(text) = store.load(args.result_id.trim()) else {
        return Ok(ToolResult::error(format!(
            "No stored result {:?}. Stored results last for one session.",
            args.result_id
        )));
    };
    let id = args.result_id.trim();
    match args.mode.as_deref().unwrap_or("lines") {
        "full" => Ok(ToolResult::text(text)),
        "lines" => Ok(ToolResult::text(retrieve::lines(
            &text, id, args.start, args.end,
        ))),
        "search" => {
            let Some(pattern) = args.pattern.as_deref().filter(|p| !p.is_empty()) else {
                return Ok(ToolResult::error("mode \"search\" needs a pattern."));
            };
            let query = retrieve::Search {
                pattern,
                regex: args.regex,
                ignore_case: args.ignore_case,
                context: args.context_chars.unwrap_or(config.context_chars),
                max: args.max_matches.unwrap_or(config.max_matches),
            };
            Ok(match retrieve::search(&text, id, &query) {
                Ok(out) => ToolResult::text(out),
                Err(err) => ToolResult::error(err),
            })
        }
        other => Ok(ToolResult::error(format!(
            "Unknown mode {other:?}; use lines, search, or full."
        ))),
    }
}

pub fn extension() -> Extension {
    let state: Shared = Arc::default();
    let (s1, s2, s3, s4, s5) = (
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
    );
    Extension::new(NAME)
        .on("session_start", move |ev| session_start(s1.clone(), ev))
        .on("session_shutdown", move |ev| session_shutdown(s2.clone(), ev))
        .on("tool_result", move |ev| tool_result(s3.clone(), ev))
        .tool(retrieve::spec(), move |args, ctx| get_tool(s4.clone(), args, ctx))
        .command(
            NAME,
            "Tool result shrinking: settings, per-tool rules, and stats (status, preview, stats, reset)",
            move |args, ctx| commands::run(s5.clone(), args, ctx),
        )
        .completions(NAME, |prefix, _host| async move { Ok(commands::complete(&prefix)) })
        .on_shutdown(move |host: Host| {
            let state = state.clone();
            async move {
                let _ = lock(&state).stats.save(&stats::path(&host.agent_dir()));
                Ok(())
            }
        })
}

pub fn main() -> ExitCode {
    extension().run()
}
