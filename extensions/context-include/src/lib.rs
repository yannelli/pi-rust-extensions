use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use pi_ext_sdk::prelude::*;

pub mod config;
pub mod fmt;
pub mod pathx;
pub mod scan;

use config::Config;
use scan::{ContextFile, Diagnostic, Scanner};

const SECTION: &str = "context_include";

#[derive(Default)]
struct Session {
    config: Config,
    cwd: Option<String>,
    last_scan: Option<Diagnostic>,
}

type Shared = Arc<Mutex<Session>>;

fn lock(state: &Shared) -> std::sync::MutexGuard<'_, Session> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

async fn load_config(host: &Host, cwd: &str) -> Config {
    let setting = match host.settings::<Value>("contextInclude").await {
        Ok(value) => value,
        Err(err) => {
            pi_ext_sdk::warn!("settings.get contextInclude failed: {}", err.message());
            None
        }
    };
    let project = config::read_file(&config::project_path(cwd.as_ref()));
    let global = config::read_file(&config::global_path(&host.agent_dir()));
    config::select(project, setting, global)
}

async fn session_start(state: Shared, ev: EventCtx) -> Result<EventReply> {
    let cwd = ev.snapshot().cwd.clone();
    let cwd = if cwd.is_empty() {
        ev.host().cwd().to_string_lossy().into_owned()
    } else {
        cwd
    };
    let config = load_config(ev.host(), &cwd).await;
    let mut session = lock(&state);
    session.config = config;
    session.cwd = Some(cwd);
    session.last_scan = None;
    Ok(EventReply::none())
}

async fn before_agent_start(state: Shared, ev: EventCtx) -> Result<EventReply> {
    let files: Vec<ContextFile> = ev
        .event()
        .pointer("/systemPromptOptions/contextFiles")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    if files.is_empty() {
        lock(&state).last_scan = None;
        return Ok(EventReply::none().with_section(SECTION, None));
    }
    let (config, cwd) = {
        let session = lock(&state);
        (session.config.clone(), session.cwd.clone())
    };
    let scanner = Scanner {
        home: std::env::var("HOME").unwrap_or_default(),
        agent_dir: ev.host().agent_dir().to_string_lossy().into_owned(),
        cwd: cwd.unwrap_or_else(|| ev.snapshot().cwd.clone()),
        config,
    };
    let output = tokio::task::spawn_blocking(move || scanner.scan(&files))
        .await
        .map_err(|err| Error::msg(format!("scan failed: {err}")))?;
    lock(&state).last_scan = Some(output.diag);
    Ok(EventReply::none().with_section(SECTION, output.section))
}

async fn status(state: Shared, ctx: CommandCtx) -> Result<()> {
    let text = {
        let session = lock(&state);
        fmt::status_text(
            session.config.max_depth,
            session.config.max_bytes,
            session.last_scan.as_ref(),
        )
    };
    ctx.host().notify_info(text).await
}

pub fn extension() -> Extension {
    let state: Shared = Arc::default();
    let (s1, s2, s3) = (state.clone(), state.clone(), state);
    Extension::new("context-include")
        .on("session_start", move |ev| session_start(s1.clone(), ev))
        .on("before_agent_start", move |ev| {
            before_agent_start(s2.clone(), ev)
        })
        .command(
            "context-include:status",
            "Show include graph: context files, resolved includes, skipped files",
            move |_args, ctx| status(s3.clone(), ctx),
        )
}

pub fn main() -> ExitCode {
    extension().run()
}
