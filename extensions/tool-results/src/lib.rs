use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, MutexGuard};

use pi_ext_sdk::prelude::*;

pub mod blobs;
pub mod clean;
mod commands;
pub mod config;
pub mod expand;
pub mod files;
mod glob;
pub mod jsonfmt;
pub mod lines;
pub mod pipeline;
mod results;
pub mod retention;
pub mod retrieve;
pub mod stack;
pub mod stats;
pub mod store;
pub mod toon;
pub mod ui;

use config::{Config, NAME};
use expand::DETAILS_KEY;
use stats::Stats;

/// The host feature that draws expanded tool rows from `render.toolResultExpanded`.
pub const EXPANDER_FEATURE: &str = "toolResultExpander";

#[derive(Default)]
pub(crate) struct Session {
    pub(crate) config: Config,
    pub(crate) stats: Stats,
    /// The trusted project file that overrides the global one.
    pub(crate) project: Option<PathBuf>,
    /// The expander settings last sent to pi: `(maxHeight, enabled)`.
    pub(crate) expander: Option<(usize, bool)>,
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

/// The global file, then a trusted project file over it; sends new expander settings to pi.
pub(crate) async fn reload(state: &Shared, host: &Host, snapshot: &Snapshot) {
    let global = files::read_file(&files::global_path(&host.agent_dir()));
    let project_path = files::project_path(&snapshot.cwd_path());
    let trusted = project_path.is_file() && project_trusted(host, snapshot).await;
    let project = trusted.then(|| files::read_file(&project_path)).flatten();
    let wanted = {
        let mut session = lock(state);
        session.project = project.is_some().then_some(project_path);
        session.config = files::load(global, project);
        let config = &session.config;
        let wanted = (config.expanded_max_height, config.expand_original);
        let sent = session.expander.unwrap_or((DEFAULT_HEIGHT, true));
        session.expander = Some(wanted);
        (wanted != sent).then_some(wanted)
    };
    let speaks = host
        .init()
        .is_some_and(|init| init.has_feature(EXPANDER_FEATURE));
    if let Some((height, enabled)) = wanted.filter(|_| speaks) {
        if let Err(err) = host
            .set_tool_result_expander(DETAILS_KEY, height, enabled)
            .await
        {
            pi_ext_sdk::warn!("register.toolResultExpander failed: {}", err.message());
        }
    }
}

/// The expander height of `initialize`, before the config is read.
const DEFAULT_HEIGHT: usize = 30;

async fn session_start(state: Shared, ev: EventCtx) -> Result<EventReply> {
    let host = ev.host().clone();
    reload(&state, &host, ev.snapshot()).await;
    let policy = {
        let mut session = lock(&state);
        if session.stats.total.is_empty() {
            session.stats = Stats::load(&stats::path(&host.agent_dir()));
        }
        session.config.retention()
    };
    let store = results::store_of(&host);
    tokio::task::spawn_blocking(move || {
        let swept = retention::sweep(&store, policy, store::now_secs());
        if !swept.removed.is_empty() {
            pi_ext_sdk::info!(
                "tool-results: retention removed {} stored results",
                swept.removed.len()
            );
        }
    });
    Ok(EventReply::none())
}

async fn session_shutdown(state: Shared, ev: EventCtx) -> Result<EventReply> {
    let _ = lock(&state)
        .stats
        .save(&stats::path(&ev.host().agent_dir()));
    Ok(EventReply::none())
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
        .on("tool_result", move |ev| results::tool_result(s3.clone(), ev))
        .tool(retrieve::spec(), move |args, ctx| {
            results::get_tool(s4.clone(), args, ctx)
        })
        .tool_result_expander(DETAILS_KEY, DEFAULT_HEIGHT, results::expanded)
        .command(
            NAME,
            "Tool result shrinking: settings, rules, and stats (status, preview, stats, list, purge, reset)",
            move |args, ctx| commands::run(s5.clone(), args, ctx),
        )
        .completions(NAME, |prefix, _host| async move {
            Ok(commands::complete(&prefix))
        })
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
