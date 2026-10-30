use serde_json::{Value, json};

use crate::agents::completion::{CallError, Completer, locale};
use crate::agents::observer;
use crate::config::{Config, resolve_observer_chunk_max_tokens};
use crate::debug_log::DebugLog;
use crate::ledger::*;
use crate::runtime::{EmptyBackoff, MemoryHost, Runtime};
use crate::serialize::serialize_source_chunk;
use crate::stages::{dropper_stage, reflector_stage};

/// What one pipeline run needs besides the host and the model.
pub struct PipelineCtx<'a> {
    pub config: &'a Config,
    pub runtime: &'a Runtime,
    pub log: &'a DebugLog,
    pub has_ui: bool,
    pub session_identity: Option<String>,
    pub memory_context_window: Option<u64>,
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Continue,
    Abort,
}

fn stage_due(entries: &[Value], current: Option<u64>, custom_type: &str, threshold: u64) -> bool {
    stage_tokens(entries, current, custom_type) >= threshold
}

/// Real growth since the stage's anchor when measurable, else the raw estimate.
pub fn stage_tokens(entries: &[Value], current: Option<u64>, custom_type: &str) -> u64 {
    current
        .and_then(|tokens| real_tokens_since_anchor(entries, Some(custom_type), tokens))
        .unwrap_or_else(|| raw_tokens_since_coverage(entries, custom_type))
}

pub fn any_stage_due(entries: &[Value], config: &Config, current: Option<u64>) -> bool {
    stage_due(
        entries,
        current,
        OM_OBSERVATIONS_RECORDED,
        config.observe_after_tokens,
    ) || stage_due(
        entries,
        current,
        OM_REFLECTIONS_RECORDED,
        config.reflect_after_tokens,
    )
}

pub(crate) async fn worker_notice<H: MemoryHost>(host: &H, ctx: &PipelineCtx<'_>, message: String) {
    if ctx.config.show_worker_notifications && ctx.has_ui {
        host.notify(message, "info").await;
    }
}

async fn stage_error<H: MemoryHost>(
    host: &H,
    ctx: &PipelineCtx<'_>,
    phase: &str,
    error: &CallError,
) {
    let message = error.to_string();
    ctx.runtime.record_stage_error(phase, &message);
    ctx.log.log(
        &format!("{phase}.error"),
        json!({ "errorMessage": message }),
    );
    if ctx.has_ui {
        host.notify(
            format!("Observational memory: {phase} failed: {message}"),
            "warning",
        )
        .await;
    }
}

pub(crate) async fn branch<H: MemoryHost>(host: &H) -> Result<Vec<Value>, CallError> {
    host.branch().await.map_err(CallError::Model)
}

/// Observer, then reflector, then dropper; a stage that aborts ends the run.
pub async fn run_pipeline<H: MemoryHost, C: Completer>(
    host: &H,
    completer: &C,
    ctx: &PipelineCtx<'_>,
) {
    ctx.runtime.set_phase(Some("observer"));
    if run_stage(host, ctx, "observer", observer_stage(host, completer, ctx)).await
        == Outcome::Abort
    {
        return;
    }
    ctx.runtime.set_phase(Some("reflector"));
    let (reflections, coverage_id) = match reflector_stage(host, completer, ctx).await {
        Ok(result) => result,
        Err(error) => return stage_error(host, ctx, "reflector", &error).await,
    };
    ctx.runtime.set_phase(Some("dropper"));
    if let Err(error) =
        dropper_stage(host, completer, ctx, &reflections, coverage_id.as_deref()).await
    {
        stage_error(host, ctx, "dropper", &error).await;
    }
}

async fn run_stage<H: MemoryHost>(
    host: &H,
    ctx: &PipelineCtx<'_>,
    phase: &str,
    stage: impl std::future::Future<Output = Result<Outcome, CallError>>,
) -> Outcome {
    match stage.await {
        Ok(outcome) => outcome,
        Err(error) => {
            stage_error(host, ctx, phase, &error).await;
            Outcome::Abort
        }
    }
}

async fn observer_stage<H: MemoryHost, C: Completer>(
    host: &H,
    completer: &C,
    ctx: &PipelineCtx<'_>,
) -> Result<Outcome, CallError> {
    let config = ctx.config;
    let entries = branch(host).await?;
    let tokens = stage_tokens(
        &entries,
        host.context_tokens().await,
        OM_OBSERVATIONS_RECORDED,
    );
    if tokens < config.observe_after_tokens {
        return Ok(Outcome::Continue);
    }
    let coverage_id = latest_coverage_marker_id(&entries, OM_OBSERVATIONS_RECORDED);
    let backoff = ctx.runtime.state().observer_empty_backoff.clone();
    if let Some(backoff) = backoff {
        let resume_at = backoff.tokens_at_empty + config.observe_after_tokens;
        if ctx.session_identity == backoff.session_identity
            && coverage_id == backoff.coverage_id
            && tokens < resume_at
        {
            ctx.log.log(
                "observer.empty_backoff",
                json!({ "tokens": tokens, "resumeAtTokens": resume_at }),
            );
            return Ok(Outcome::Continue);
        }
        ctx.runtime.state().observer_empty_backoff = None;
    }

    let last = latest_coverage_index(&entries, OM_OBSERVATIONS_RECORDED);
    let backlog: Vec<Value> = entries
        .iter()
        .skip((last + 1) as usize)
        .filter(|e| is_source_entry(e))
        .cloned()
        .collect();
    let max_chunk = resolve_observer_chunk_max_tokens(config, ctx.memory_context_window);
    let chunk = serialize_source_chunk(&backlog, Some(max_chunk));
    let Some(covers_up_to_id) = chunk.source_entry_ids.last().cloned() else {
        return Ok(Outcome::Continue);
    };
    if chunk.text.trim().is_empty() {
        return Ok(Outcome::Continue);
    }
    if chunk.source_entry_ids.len() < backlog.len() || !chunk.truncated_source_entry_ids.is_empty()
    {
        ctx.log.log("observer.chunk_capped", json!({
            "maxChunkTokens": max_chunk, "backlogEntries": backlog.len(), "backlogTokens": tokens,
            "chunkEntries": chunk.source_entry_ids.len(), "chunkTokens": chunk.estimated_tokens,
            "truncatedSourceEntryIds": chunk.truncated_source_entry_ids,
        }));
    }

    let memory = full_projection(&entries, None);
    let prior_reflections: Vec<String> = memory
        .reflections
        .iter()
        .map(reflection_to_summary_line)
        .collect();
    let prior_observations: Vec<String> = memory
        .observations
        .iter()
        .map(observation_to_summary_line)
        .collect();
    worker_notice(
        host,
        ctx,
        format!(
            "Observational memory: observer running on ~{}-token chunk",
            locale(chunk.estimated_tokens)
        ),
    )
    .await;
    ctx.log.log("observer.start", json!({
        "tokens": tokens, "chunkTokens": chunk.estimated_tokens, "coversUpToId": covers_up_to_id,
        "sourceEntryIds": chunk.source_entry_ids, "sourceEntryCount": chunk.source_entry_ids.len(),
    }));

    let input = observer::ObserverInput {
        prior_reflections: &prior_reflections,
        prior_observations: &prior_observations,
        chunk: &chunk.text,
        allowed_source_entry_ids: &chunk.source_entry_ids,
    };
    let observations = observer::run_observer(completer, &input).await?;
    if observations.is_empty() {
        ctx.log
            .log("observer.empty", json!({ "coversUpToId": covers_up_to_id }));
        ctx.runtime.state().observer_empty_backoff = Some(EmptyBackoff {
            session_identity: ctx.session_identity.clone(),
            coverage_id,
            tokens_at_empty: tokens,
        });
        worker_notice(host, ctx, "Observational memory: observer found nothing new in this chunk (coverage unchanged; will retry later)".to_owned()).await;
        return Ok(Outcome::Continue);
    }
    let count = observations.len();
    let data = ObservationsRecorded {
        observations,
        covers_up_to_id: covers_up_to_id.clone(),
    };
    host.append(
        OM_OBSERVATIONS_RECORDED,
        serde_json::to_value(&data).unwrap_or_default(),
    )
    .await
    .map_err(CallError::Model)?;
    ctx.log.log(
        "observer.appended",
        json!({ "count": count, "coversUpToId": covers_up_to_id }),
    );
    let plural = if count == 1 { "" } else { "s" };
    worker_notice(
        host,
        ctx,
        format!("Observational memory: {count} observation{plural} recorded"),
    )
    .await;
    Ok(Outcome::Continue)
}
