use std::sync::Arc;

use pi_ext_sdk::prelude::*;
use tokio::sync::Mutex;

use crate::{
    config, diagnostics, pipeline,
    state::{Guard, State},
    tools::bounded,
};

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
    if (ignored || untrusted) && !state.warned_trust {
        state.warned_trust = true;
        drop(state);
        let text = "Lens does not auto-detect tools or use executable project settings because the project is untrusted.";
        if host.has_ui() {
            host.notify_warning(text).await?;
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
            let guard = host
                .entries_of::<Guard>("lens-read-guard")
                .await?
                .pop()
                .unwrap_or_default();
            let mut state = state.lock().await;
            state.guard = guard;
            state.findings.clear();
        }
        "tool_call" => {
            let mut state = state.lock().await;
            let tool = ctx.event()["toolName"].as_str().unwrap_or_default();
            if state.config.enabled && state.config.read_guard && matches!(tool, "write" | "edit") {
                if let Some(path) = ctx.event()["input"]["path"].as_str() {
                    let path = diagnostics::normalize_path(&host.cwd(), path);
                    if let Some(reason) =
                        state.guard.block_reason(&path, tool, &ctx.event()["input"])
                    {
                        return Ok(EventReply::block(reason));
                    }
                }
            }
        }
        "tool_result" => {
            if ctx.event()["isError"] == true {
                return Ok(EventReply::none());
            }
            let tool = ctx.event()["toolName"].as_str().unwrap_or_default();
            let Some(path) = ctx.event()["input"]["path"].as_str() else {
                return Ok(EventReply::none());
            };
            let cwd = host.cwd();
            let path = diagnostics::normalize_path(&cwd, path);
            let config = state.lock().await.config.clone();
            if !config.enabled {
                return Ok(EventReply::none());
            }
            let state = match tool {
                "read" => {
                    let content = ctx.event()["content"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    let mut state = state.lock().await;
                    state.guard.record(&path, &ctx.event()["input"], &content);
                    state
                }
                "write" | "edit" => {
                    let findings = if diagnostics::within(&cwd, &path) {
                        let work = pipeline::run(&config, &cwd, &path);
                        Some(
                            tokio::select! { result = work => result, _ = ctx.cancelled() => return Err(Error::cancelled()) },
                        )
                    } else {
                        None
                    };
                    let mut state = state.lock().await;
                    if let Some(findings) = findings {
                        state.findings.insert(path.clone(), findings);
                    }
                    state.guard.invalidate(&path);
                    state
                }
                _ => return Ok(EventReply::none()),
            };
            let guard = state.guard.clone();
            let count = state.all_findings().len();
            drop(state);
            host.append_entry("lens-read-guard", &guard).await?;
            if host.has_ui() {
                host.set_widget(
                    "pi-lens",
                    [format!("Lens: {count} findings")],
                    Placement::BelowEditor,
                )
                .await?;
            }
        }
        "context" => {
            let state = state.lock().await;
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
