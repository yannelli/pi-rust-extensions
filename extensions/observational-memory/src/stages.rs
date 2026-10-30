use serde_json::json;

use crate::agents::completion::{CallError, Completer, locale};
use crate::agents::coverage::pool_metrics;
use crate::agents::{dropper, reflector};
use crate::ledger::*;
use crate::pipeline::{
    PipelineCtx, append_on_branch, backoff_resume_at, branch, reflector_slot, stage_tokens,
    start_backoff, worker_notice,
};
use crate::runtime::MemoryHost;

pub(crate) type Reflected = (Vec<Reflection>, Option<String>);

pub(crate) async fn reflector_stage<H: MemoryHost, C: Completer>(
    host: &H,
    completer: &C,
    ctx: &PipelineCtx<'_>,
) -> Result<Reflected, CallError> {
    let entries = branch(host).await?;
    let tokens = stage_tokens(
        &entries,
        host.context_tokens().await,
        OM_REFLECTIONS_RECORDED,
    );
    let none = (Vec::new(), None);
    if tokens < ctx.config.reflect_after_tokens {
        return Ok(none);
    }
    let Some(observation_coverage_id) =
        latest_coverage_marker_id(&entries, OM_OBSERVATIONS_RECORDED)
    else {
        return Ok(none);
    };
    let reflection_coverage_id = latest_coverage_marker_id(&entries, OM_REFLECTIONS_RECORDED);
    if let Some(resume_at) = backoff_resume_at(
        ctx,
        reflector_slot,
        &reflection_coverage_id,
        tokens,
        ctx.config.reflect_after_tokens,
    ) {
        ctx.log.log(
            "reflector.backoff",
            json!({ "tokens": tokens, "resumeAtTokens": resume_at }),
        );
        return Ok(none);
    }
    worker_notice(
        host,
        ctx,
        format!(
            "Observational memory: reflector running (~{} tokens)",
            locale(tokens)
        ),
    )
    .await;
    let folded = fold_ledger(&entries, None);
    let reflected =
        reflector::run_reflector(completer, &folded.reflections, &folded.active_observations).await;
    let reflections = match reflected {
        Ok(reflections) if !reflections.is_empty() => reflections,
        other => {
            start_backoff(ctx, reflector_slot, reflection_coverage_id, tokens);
            return other.map(|_| none);
        }
    };
    let data = ReflectionsRecorded {
        reflections,
        covers_up_to_id: observation_coverage_id,
    };
    let value = serde_json::to_value(&data).unwrap_or_default();
    if !append_on_branch(
        host,
        ctx,
        OM_REFLECTIONS_RECORDED,
        &data.covers_up_to_id,
        value,
    )
    .await?
    {
        return Ok(none);
    }
    Ok((data.reflections, Some(data.covers_up_to_id)))
}

pub(crate) async fn dropper_stage<H: MemoryHost, C: Completer>(
    host: &H,
    completer: &C,
    ctx: &PipelineCtx<'_>,
    same_run: &[Reflection],
    same_run_coverage_id: Option<&str>,
) -> Result<(), CallError> {
    let Some(reflection_coverage_id) = same_run_coverage_id.filter(|_| !same_run.is_empty()) else {
        ctx.log.log(
            "dropper.waiting_for_reflection",
            json!({ "sameRunReflections": same_run.len() }),
        );
        return Ok(());
    };
    let entries = branch(host).await?;
    let Some(observation_coverage_id) =
        latest_coverage_marker_id(&entries, OM_OBSERVATIONS_RECORDED)
    else {
        return Ok(());
    };
    let folded = fold_ledger(&entries, None);
    let target = ctx.config.observations_pool_target_tokens;
    let metrics = pool_metrics(&folded.active_observations, target);
    if !metrics.ready {
        ctx.log.log(
            "dropper.not_ready",
            json!({ "observationTokens": metrics.observation_tokens, "targetTokens": target }),
        );
        return Ok(());
    }
    worker_notice(host, ctx, format!(
        "Observational memory: dropper running after reflection — active observation pool ~{} / {} target tokens ({}%)",
        locale(metrics.observation_tokens), locale(target), locale((metrics.fullness * 100.0).round() as u64)
    ))
    .await;
    let mut reflections = folded.reflections.clone();
    for r in same_run {
        if !reflections.iter().any(|existing| existing.id == r.id) {
            reflections.push(r.clone());
        }
    }
    let dropped =
        dropper::run_dropper(completer, &reflections, &folded.active_observations, target).await?;
    let covers = earlier_coverage_marker_id(
        &entries,
        Some(&observation_coverage_id),
        Some(reflection_coverage_id),
    );
    ctx.log.log(
        "dropper.append",
        json!({ "droppedIdsCount": dropped.len(), "coversUpToId": covers }),
    );
    if let (Some(covers_up_to_id), false) = (covers, dropped.is_empty()) {
        let data = ObservationsDropped {
            observation_ids: dropped,
            covers_up_to_id,
        };
        let value = serde_json::to_value(&data).unwrap_or_default();
        append_on_branch(
            host,
            ctx,
            OM_OBSERVATIONS_DROPPED,
            &data.covers_up_to_id,
            value,
        )
        .await?;
    }
    Ok(())
}
