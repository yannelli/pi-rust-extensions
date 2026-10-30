use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pi_ext_sdk::prelude::*;
use tokio::sync::Mutex;

use crate::{
    config, diagnostics, pipeline,
    state::{self, Guard, State},
    tools::bounded,
};

/// Limit for each call to pi from a hook, for the state lock in a hook, and for the
/// file read of the read guard. On expiry the hook skips that step.
pub(crate) const HOST_LIMIT: Duration = Duration::from_secs(2);

/// Runs `call` within [`HOST_LIMIT`]. An error or a timeout becomes a log line and `None`.
pub(crate) async fn bounded_call<T>(
    what: &str,
    call: impl Future<Output = Result<T>>,
) -> Option<T> {
    match tokio::time::timeout(HOST_LIMIT, call).await {
        Ok(Ok(value)) => Some(value),
        Ok(Err(error)) => {
            pi_ext_sdk::debug!("lens: {what} failed: {error}");
            None
        }
        Err(_) => {
            pi_ext_sdk::warn!(
                "lens: {what} did not finish in {} ms",
                HOST_LIMIT.as_millis()
            );
            None
        }
    }
}

async fn file_text(path: &Path) -> Option<String> {
    let path = path.to_path_buf();
    let read = tokio::task::spawn_blocking(move || state::small_text(&path));
    tokio::time::timeout(HOST_LIMIT, read).await.ok()?.ok()?
}

pub(crate) async fn refresh(host: &Host, state: &Arc<Mutex<State>>) -> Result<()> {
    let (mut config, ignored) =
        config::load(&host.agent_dir(), &host.cwd(), config::trusted(host).await)?;
    if let Some(snapshot) = host.snapshot() {
        config.enabled &= !snapshot.flag_bool("no-lens");
        config.context &= !snapshot.flag_bool("no-lens-context");
        config.read_guard &= !snapshot.flag_bool("no-read-guard");
    }
    config.context &= std::env::var("PI_LENS_NO_CONTEXT_INJECTION")
        .ok()
        .as_deref()
        != Some("1");
    let untrusted = !config.trusted && config.enabled && config.auto_detect;
    let mut state = state.lock().await;
    state.config = config;
    state.runner.reset();
    if (ignored || untrusted) && !state.warned_trust {
        state.warned_trust = true;
        drop(state);
        let text = "Lens does not auto-detect tools or use executable project settings because the project is untrusted.";
        if host.has_ui() {
            bounded_call("trust notice", host.notify_warning(text)).await;
        } else {
            pi_ext_sdk::warn!("{text}");
        }
    }
    Ok(())
}

pub(crate) async fn handle(ctx: EventCtx, state: Arc<Mutex<State>>) -> Result<EventReply> {
    let host = ctx.host();
    match ctx.name() {
        "session_start" | "session_tree" | "session_compact" => {
            refresh(host, &state).await?;
            let guard = bounded_call(
                "loading lens-read-guard entries",
                host.entries_of::<Guard>("lens-read-guard"),
            )
            .await
            .and_then(|mut entries| entries.pop())
            .unwrap_or_default();
            let mut state = state.lock().await;
            state.guard = guard;
            state.findings.clear();
        }
        "tool_call" => {
            let tool = ctx.event()["toolName"].as_str().unwrap_or_default();
            let Some(path) = ctx.event()["input"]["path"].as_str() else {
                return Ok(EventReply::none());
            };
            if !matches!(tool, "write" | "edit") {
                return Ok(EventReply::none());
            }
            let path = diagnostics::normalize_path(&host.cwd(), path);
            let text = file_text(&path).await;
            let Ok(mut state) = tokio::time::timeout(HOST_LIMIT, state.lock()).await else {
                pi_ext_sdk::warn!("lens: read guard skipped; state is busy");
                return Ok(EventReply::none());
            };
            if state.config.enabled && state.config.read_guard {
                if let Some(reason) =
                    state
                        .guard
                        .block_reason(&path, text, tool, &ctx.event()["input"])
                {
                    return Ok(EventReply::block(reason));
                }
            }
        }
        "tool_result" => {
            if ctx.event()["isError"] == true {
                return Ok(EventReply::none());
            }
            let tool = ctx.event()["toolName"].as_str().unwrap_or_default();
            if !matches!(tool, "read" | "write" | "edit") {
                return Ok(EventReply::none());
            }
            let Some(path) = ctx.event()["input"]["path"].as_str() else {
                return Ok(EventReply::none());
            };
            let path = diagnostics::normalize_path(&host.cwd(), path);
            let Ok(locked) = tokio::time::timeout(HOST_LIMIT, state.lock()).await else {
                pi_ext_sdk::warn!("lens: scan skipped; state is busy");
                return Ok(EventReply::none());
            };
            let config = locked.config.clone();
            drop(locked);
            if !config.enabled {
                return Ok(EventReply::none());
            }
            let limit = Duration::from_millis(config.pipeline_timeout_ms) + 2 * HOST_LIMIT;
            let work = after_tool(&ctx, &state, &config, tool, &path);
            tokio::select! {
                result = tokio::time::timeout(limit, work) => {
                    if result.is_err() {
                        pi_ext_sdk::warn!("lens: scan of {} stopped after {} ms", path.display(), limit.as_millis());
                    }
                }
                _ = ctx.cancelled() => return Err(Error::cancelled()),
            }
        }
        "context" => {
            let Ok(state) = tokio::time::timeout(HOST_LIMIT, state.lock()).await else {
                return Ok(EventReply::none());
            };
            if state.config.enabled && state.config.context {
                let findings = state.all_findings();
                if !findings.is_empty() {
                    let mut messages = ctx.event()["messages"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    let text = bounded(format!(
                        "[Lens diagnostics]\n{}",
                        serde_json::to_string(&findings)?
                    ));
                    messages.push(json!({"role":"user","content":[{"type":"text","text":text}],"timestamp":0}));
                    return Ok(EventReply::result(json!({"messages":messages})));
                }
            }
        }
        _ => {}
    }
    Ok(EventReply::none())
}

async fn after_tool(
    ctx: &EventCtx,
    state: &Arc<Mutex<State>>,
    config: &config::Config,
    tool: &str,
    path: &Path,
) {
    let host = ctx.host();
    let cwd = host.cwd();
    let runner = state.lock().await.runner.clone();
    let state = if tool == "read" {
        let content = ctx.event()["content"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let text = file_text(path).await;
        let mut state = state.lock().await;
        state
            .guard
            .record(path, text, &ctx.event()["input"], &content);
        state
    } else {
        let findings = if diagnostics::within(&cwd, path) {
            Some(pipeline::run(config, &cwd, path, &runner).await)
        } else {
            None
        };
        let mut state = state.lock().await;
        if let Some(findings) = findings {
            state.findings.insert(path.to_path_buf(), findings);
        }
        state.guard.invalidate(path);
        state
    };
    let guard = state.guard.clone();
    let count = state.all_findings().len();
    drop(state);
    bounded_call(
        "saving lens-read-guard",
        host.append_entry("lens-read-guard", &guard),
    )
    .await;
    if host.has_ui() {
        let lines = [format!("Lens: {count} findings")];
        bounded_call(
            "updating the Lens widget",
            host.set_widget("pi-lens", lines, Placement::BelowEditor),
        )
        .await;
    }
}
