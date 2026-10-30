use std::collections::HashMap;

use chrono::NaiveDateTime;
use serde_json::Value;

use super::completion::{CallError, Completer, array_field, call_json, join_or_empty, locale};
use super::coverage::{
    PoolMetrics, coverage_drop_rank, coverage_map, observation_coverage_line, pool_metrics, tier_of,
};
use crate::ledger::{Observation, Reflection, reflection_to_summary_line};
use crate::prompts::dropper::DROPPER_SYSTEM;

fn relevance_rank(relevance: &str) -> u8 {
    match relevance {
        "low" => 0,
        "medium" => 1,
        "high" => 2,
        _ => 3,
    }
}

fn timestamp_rank(timestamp: &str) -> i64 {
    NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M")
        .map_or(i64::MAX, |t| t.and_utc().timestamp())
}

pub fn parse_reply(value: &Value) -> Result<Vec<String>, String> {
    let ids = array_field(value, "ids")?;
    ids.iter()
        .map(|id| {
            id.as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| "\"ids\" must hold non-empty strings".to_owned())
        })
        .collect()
}

/// Known ids in first-proposal order, without duplicates.
pub fn queue_candidates(ids: &[String], observations: &[Observation]) -> Vec<String> {
    let mut queued: Vec<String> = Vec::new();
    for id in ids {
        if observations.iter().any(|o| &o.id == id) && !queued.contains(id) {
            queued.push(id.clone());
        }
    }
    queued
}

/// Orders by coverage (strong first), relevance (low first), age, then proposal order.
pub fn select_drop_candidates(
    ids: &[String],
    observations: &[Observation],
    max_drops: usize,
    reflections: &[Reflection],
) -> Vec<String> {
    if max_drops == 0 || ids.is_empty() {
        return Vec::new();
    }
    let by_id: HashMap<&str, &Observation> =
        observations.iter().map(|o| (o.id.as_str(), o)).collect();
    let coverage = coverage_map(observations, reflections);
    let mut candidates: Vec<(usize, &Observation)> = queue_candidates(ids, observations)
        .iter()
        .enumerate()
        .filter_map(|(i, id)| by_id.get(id.as_str()).map(|o| (i, *o)))
        .collect();
    candidates.sort_by_key(|(index, o)| {
        (
            coverage_drop_rank(tier_of(o, &coverage)),
            relevance_rank(&o.relevance),
            timestamp_rank(&o.timestamp),
            *index,
        )
    });
    candidates
        .into_iter()
        .take(max_drops)
        .map(|(_, o)| o.id.clone())
        .collect()
}

pub fn user_prompt(
    reflections: &[Reflection],
    observations: &[Observation],
    m: &PoolMetrics,
) -> String {
    let coverage = coverage_map(observations, reflections);
    let reflection_lines: Vec<String> =
        reflections.iter().map(reflection_to_summary_line).collect();
    let observation_lines: Vec<String> = observations
        .iter()
        .map(|o| observation_coverage_line(o, tier_of(o, &coverage)))
        .collect();
    let plural = if m.max_drops_allowed == 1 { "" } else { "s" };
    format!(
        "CURRENT REFLECTIONS:\n{}\n\nCURRENT OBSERVATIONS:\n{}\n\nActive observation pool: ~{} tokens; target: ~{} tokens; fullness against target: ~{}%; over target by ~{} tokens.\nMaximum drops allowed this run: {} observation{plural}. This maximum is sized to move the active pool toward the target if every proposed drop is clearly safe.\nThis maximum is a hard upper bound, not a target. Drop fewer or none if fewer observations are clearly safe.",
        join_or_empty(&reflection_lines),
        join_or_empty(&observation_lines),
        locale(m.observation_tokens),
        locale(m.target_tokens),
        locale((m.fullness * 100.0).round() as u64),
        locale(m.tokens_over_target),
        locale(m.max_drops_allowed as u64),
    )
}

/// Observation ids to drop; empty when nothing is safe or the pool is under target.
pub async fn run_dropper<C: Completer>(
    completer: &C,
    reflections: &[Reflection],
    observations: &[Observation],
    target_tokens: u64,
) -> Result<Vec<String>, CallError> {
    let metrics = pool_metrics(observations, target_tokens);
    if observations.is_empty() || metrics.max_drops_allowed == 0 {
        return Ok(Vec::new());
    }
    let prompt = user_prompt(reflections, observations, &metrics);
    let proposed = call_json(completer, DROPPER_SYSTEM, &prompt, parse_reply).await?;
    Ok(select_drop_candidates(
        &proposed,
        observations,
        metrics.max_drops_allowed,
        reflections,
    ))
}
