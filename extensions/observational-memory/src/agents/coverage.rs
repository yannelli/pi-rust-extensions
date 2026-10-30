use std::collections::{HashMap, HashSet};

use crate::ledger::{Observation, Reflection};
use crate::tokens::observation_line_token_count;

pub fn reflection_support_counts(reflections: &[Reflection]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for r in reflections {
        let unique: HashSet<&String> = r.supporting_observation_ids.iter().collect();
        for id in unique {
            *counts.entry(id.clone()).or_insert(0) += 1;
        }
    }
    counts
}

pub fn tier_for_count(count: usize) -> &'static str {
    match count {
        0 => "none",
        1 => "partial",
        _ => "strong",
    }
}

pub fn coverage_map(
    observations: &[Observation],
    reflections: &[Reflection],
) -> HashMap<String, &'static str> {
    let counts = reflection_support_counts(reflections);
    observations
        .iter()
        .map(|o| {
            (
                o.id.clone(),
                tier_for_count(counts.get(&o.id).copied().unwrap_or(0)),
            )
        })
        .collect()
}

pub fn tier_of<'a>(o: &Observation, map: &HashMap<String, &'a str>) -> &'a str {
    map.get(&o.id).copied().unwrap_or("none")
}

pub fn coverage_drop_rank(tier: &str) -> u8 {
    match tier {
        "strong" => 0,
        "partial" => 1,
        _ => 2,
    }
}

/// `[id] timestamp [relevance] [coverage: tier] content`, shared by reflector and dropper.
pub fn observation_coverage_line(o: &Observation, tier: &str) -> String {
    format!(
        "[{}] {} [{}] [coverage: {tier}] {}",
        o.id, o.timestamp, o.relevance, o.content
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct PoolMetrics {
    pub observation_tokens: u64,
    pub target_tokens: u64,
    pub tokens_over_target: u64,
    pub fullness: f64,
    pub active_observation_count: usize,
    pub max_drops_allowed: usize,
    pub over_target: bool,
    pub ready: bool,
}

pub fn observation_token_sum(observations: &[Observation]) -> u64 {
    observations.iter().map(observation_line_token_count).sum()
}

pub fn max_drop_count(count: usize, observation_tokens: u64, target_tokens: u64) -> usize {
    if count == 0 || observation_tokens == 0 || observation_tokens <= target_tokens {
        return 0;
    }
    let average = observation_tokens as f64 / count as f64;
    let estimated = ((observation_tokens - target_tokens) as f64 / average).ceil() as usize;
    estimated.max(1).min(count)
}

pub fn pool_metrics(observations: &[Observation], target_tokens: u64) -> PoolMetrics {
    let observation_tokens = observation_token_sum(observations);
    let fullness = if observation_tokens == 0 || target_tokens == 0 {
        0.0
    } else {
        observation_tokens as f64 / target_tokens as f64
    };
    let max_drops_allowed = max_drop_count(observations.len(), observation_tokens, target_tokens);
    let over_target = observation_tokens > target_tokens;
    PoolMetrics {
        observation_tokens,
        target_tokens,
        tokens_over_target: observation_tokens.saturating_sub(target_tokens),
        fullness,
        active_observation_count: observations.len(),
        max_drops_allowed,
        over_target,
        ready: over_target && max_drops_allowed > 0,
    }
}
