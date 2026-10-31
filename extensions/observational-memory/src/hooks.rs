use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use pi_ext_sdk::NotifyType;
use pi_ext_sdk::prelude::*;

use crate::State;
use crate::config::resolve_compact_after_tokens;
use crate::debug_log::{DebugLog, debug_log_path};
use crate::host_glue::{HostCompleter, HostMemory, resolve_memory_model, state_dir};
use crate::ledger::{
    build_compaction_projection, raw_tokens_since_last_compaction, render_summary,
};
use crate::pipeline::{PipelineCtx, any_stage_due, run_pipeline};
use crate::runtime::MemoryHost;

fn run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "consolidation-{:x}-{:06x}",
        nanos / 1_000_000,
        nanos % 0x100_0000
    )
}

/// `agent_start` / `turn_end`: starts the pipeline in a background task when a stage is due
/// and no compaction runs.
pub fn launch_consolidation(state: Arc<State>, ev: &EventCtx) {
    let host = ev.host().clone();
    let snapshot = ev.snapshot().clone();
    tokio::spawn(async move {
        let config = state.config(&host, &snapshot).await;
        if config.passive {
            return;
        }
        let runtime = &state.runtime;
        let Some(_running) = runtime.claim(&runtime.consolidation_in_flight) else {
            return;
        };
        if runtime.compacting() {
            return;
        }
        let memory = HostMemory(host.clone());
        let Ok(entries) = memory.branch().await else {
            return;
        };
        if !any_stage_due(&entries, &config, memory.context_tokens().await) {
            return;
        }
        runtime.begin_consolidation();
        let model = resolve_memory_model(&host, &config, &snapshot).await;
        let log = DebugLog {
            path: config.debug_log.then(|| {
                debug_log_path(
                    &state_dir(&host.agent_dir()),
                    snapshot.session_id.as_deref(),
                )
            }),
            cwd: snapshot.cwd.clone(),
            session_id: snapshot.session_id.clone(),
            session_file: snapshot.session_file.clone(),
            run_id: Some(run_id()),
        };
        let ctx = PipelineCtx {
            config: &config,
            runtime,
            log: &log,
            has_ui: snapshot.has_ui,
            session_identity: snapshot
                .session_id
                .clone()
                .or(snapshot.session_file.clone()),
            memory_context_window: model.context_window,
        };
        let completer = HostCompleter::new(host.clone(), &model, &config);
        run_pipeline(&memory, &completer, &ctx).await;
        runtime.set_phase(None);
    });
}

/// `agent_settled`: compacts in a background task once the source tokens reach the threshold
/// and no consolidation runs.
pub fn launch_compaction(state: Arc<State>, ev: &EventCtx) {
    let host = ev.host().clone();
    let snapshot = ev.snapshot().clone();
    tokio::spawn(async move {
        let config = state.config(&host, &snapshot).await;
        if config.passive {
            return;
        }
        let runtime = &state.runtime;
        let Some(_running) = runtime.claim(&runtime.compact_in_flight) else {
            return;
        };
        runtime.wait_for_consolidation().await;
        let latest = host.snapshot();
        let current = latest.as_deref().unwrap_or(&snapshot);
        if current.session_id != snapshot.session_id
            || current.session_file != snapshot.session_file
            || current.cwd != snapshot.cwd
        {
            return;
        }
        let Ok(entries) = host.branch(None).await else {
            return;
        };
        let progress = raw_tokens_since_last_compaction(&entries);
        let window = current.model.as_ref().and_then(|m| m.context_window);
        let threshold = resolve_compact_after_tokens(&config, window);
        if progress < threshold {
            return;
        }
        let has_ui = current.has_ui;
        let notify = |message: String, kind: NotifyType| {
            let host = host.clone();
            async move {
                if has_ui {
                    let _ = host.notify_as(message, kind).await;
                }
            }
        };
        notify(format!(
            "Observational memory: compaction threshold reached (~{} estimated source tokens); triggering compaction",
            crate::agents::completion::locale(progress)
        ), NotifyType::Info).await;
        tokio::task::yield_now().await;
        if !host.is_idle().await.unwrap_or(false) {
            return notify(
                "Observational memory: compaction deferred — agent became busy before compaction"
                    .into(),
                NotifyType::Info,
            )
            .await;
        }
        let current = host
            .branch(None)
            .await
            .map(|e| raw_tokens_since_last_compaction(&e))
            .unwrap_or(0);
        if current < threshold {
            return notify("Observational memory: compaction skipped — another compaction already ran before deferred compaction".into(), NotifyType::Info).await;
        }
        match host.compact(None).await {
            Ok(_) => {
                notify(
                    "Observational memory: compaction complete".into(),
                    NotifyType::Info,
                )
                .await
            }
            Err(error) if error.message() == "Compaction cancelled" => {}
            Err(error) => {
                notify(
                    format!("Observational memory: {}", error.message()),
                    NotifyType::Error,
                )
                .await
            }
        }
    });
}

/// `session_before_compact`: the memory projection becomes the compaction summary.
pub async fn before_compact(state: Arc<State>, ev: EventCtx) -> Result<EventReply> {
    let runtime = &state.runtime;
    let Some(_running) = runtime.claim(&runtime.compact_hook_in_flight) else {
        if ev.snapshot().has_ui {
            let host = ev.host().clone();
            tokio::spawn(async move {
                let message = "Observational memory: another compaction is already in progress; cancelling duplicate";
                let _ = host.notify_warning(message).await;
            });
        }
        return Ok(EventReply::cancel());
    };
    let config = state.config(ev.host(), ev.snapshot()).await;
    let event = ev.event();
    let preparation = &event["preparation"];
    let Some(first_kept) = preparation["firstKeptEntryId"].as_str() else {
        return Ok(EventReply::none());
    };
    let entries = event["branchEntries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let projection =
        build_compaction_projection(&entries, first_kept, config.observations_pool_max_tokens);
    let summary = render_summary(
        &projection.projection.reflections,
        &projection.projection.observations,
    );
    if summary.is_empty() {
        return Ok(EventReply::none());
    }
    Ok(EventReply::result(json!({
        "compaction": {
            "summary": summary,
            "firstKeptEntryId": first_kept,
            "tokensBefore": preparation["tokensBefore"],
            "details": projection.details,
        }
    })))
}
