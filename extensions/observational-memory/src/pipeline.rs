use serde_json::{Value, json};

use crate::agents::completion::{CallError, Completer, locale};
use crate::agents::observer;
use crate::config::{Config, resolve_observer_chunk_max_tokens};
use crate::debug_log::DebugLog;
use crate::ledger::*;
use crate::runtime::{EmptyBackoff, MemoryHost, RunState, Runtime};
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

pub(crate) type BackoffSlot = fn(&mut RunState) -> &mut Option<EmptyBackoff>;

pub(crate) fn observer_slot(state: &mut RunState) -> &mut Option<EmptyBackoff> {
    &mut state.observer_empty_backoff
}

pub(crate) fn reflector_slot(state: &mut RunState) -> &mut Option<EmptyBackoff> {
    &mut state.reflector_backoff
}

/// The token count where a stage resumes, while its backoff holds; clears a stale backoff.
pub(crate) fn backoff_resume_at(
    ctx: &PipelineCtx<'_>,
    slot: BackoffSlot,
    coverage_id: &Option<String>,
    tokens: u64,
    threshold: u64,
) -> Option<u64> {
    let mut state = ctx.runtime.state();
    let backoff = slot(&mut state);
    let held = backoff.as_ref().and_then(|b| {
        let resume_at = b.tokens_at_empty.saturating_add(threshold);
        (ctx.session_identity == b.session_identity
            && *coverage_id == b.coverage_id
            && tokens < resume_at)
            .then_some(resume_at)
    });
    if held.is_none() {
        *backoff = None;
    }
    held
}

pub(crate) fn start_backoff(
    ctx: &PipelineCtx<'_>,
    slot: BackoffSlot,
    coverage_id: Option<String>,
    tokens: u64,
) {
    *slot(&mut ctx.runtime.state()) = Some(EmptyBackoff {
        session_identity: ctx.session_identity.clone(),
        coverage_id,
        tokens_at_empty: tokens,
    });
}

/// Appends a ledger record when `covers_up_to_id` is still on the branch; returns whether it did.
/// The branch can change during a long completion (`/tree` navigation).
pub(crate) async fn append_on_branch<H: MemoryHost>(
    host: &H,
    ctx: &PipelineCtx<'_>,
    custom_type: &str,
    covers_up_to_id: &str,
    data: Value,
) -> Result<bool, CallError> {
    let entries = branch(host).await?;
    if !entries.iter().any(|e| entry_id(e) == covers_up_to_id) {
        ctx.log.log(
            "append.branch_changed",
            json!({ "customType": custom_type, "coversUpToId": covers_up_to_id }),
        );
        return Ok(false);
    }
    host.append(custom_type, data)
        .await
        .map_err(CallError::Model)?;
    Ok(true)
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
    if let Some(resume_at) = backoff_resume_at(
        ctx,
        observer_slot,
        &coverage_id,
        tokens,
        config.observe_after_tokens,
    ) {
        ctx.log.log(
            "observer.empty_backoff",
            json!({ "tokens": tokens, "resumeAtTokens": resume_at }),
        );
        return Ok(Outcome::Continue);
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
    let observations = match observer::run_observer(completer, &input).await {
        Ok(observations) => observations,
        Err(error) => {
            start_backoff(ctx, observer_slot, coverage_id, tokens);
            return Err(error);
        }
    };
    if observations.is_empty() {
        ctx.log
            .log("observer.empty", json!({ "coversUpToId": covers_up_to_id }));
        start_backoff(ctx, observer_slot, coverage_id, tokens);
        worker_notice(host, ctx, "Observational memory: observer found nothing new in this chunk (coverage unchanged; will retry later)".to_owned()).await;
        return Ok(Outcome::Continue);
    }
    let count = observations.len();
    let data = ObservationsRecorded {
        observations,
        covers_up_to_id: covers_up_to_id.clone(),
    };
    let data = serde_json::to_value(&data).unwrap_or_default();
    if !append_on_branch(host, ctx, OM_OBSERVATIONS_RECORDED, &covers_up_to_id, data).await? {
        return Ok(Outcome::Abort);
    }
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
