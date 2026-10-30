use std::collections::HashMap;

use serde_json::Value;

use super::types::{
    OM_OBSERVATIONS_DROPPED, OM_OBSERVATIONS_RECORDED, OM_REFLECTIONS_RECORDED, coverage_marker,
    entry_id, entry_type,
};
use crate::tokens::estimate_entry_tokens;

pub fn is_source_entry(entry: &Value) -> bool {
    matches!(
        entry_type(entry),
        "message" | "custom_message" | "branch_summary"
    )
}

pub fn index_by_id(entries: &[Value]) -> HashMap<&str, usize> {
    entries
        .iter()
        .enumerate()
        .map(|(i, e)| (entry_id(e), i))
        .collect()
}

pub fn index_of(entries: &[Value], id: Option<&str>) -> Option<usize> {
    let id = id.filter(|id| !id.is_empty())?;
    entries.iter().rposition(|e| entry_id(e) == id)
}

fn latest_coverage(entries: &[Value], custom_type: &str) -> Option<(usize, String)> {
    let indexes = index_by_id(entries);
    let mut latest: Option<(usize, String)> = None;
    for entry in entries {
        let Some(marker) = coverage_marker(entry, custom_type) else {
            continue;
        };
        let Some(&covered) = indexes.get(marker) else {
            continue;
        };
        if latest.as_ref().is_none_or(|(i, _)| covered > *i) {
            latest = Some((covered, marker.to_owned()));
        }
    }
    latest
}

/// Index of the latest covered entry, or `-1` when nothing is covered.
pub fn latest_coverage_index(entries: &[Value], custom_type: &str) -> i64 {
    latest_coverage(entries, custom_type).map_or(-1, |(i, _)| i as i64)
}

pub fn latest_coverage_marker_id(entries: &[Value], custom_type: &str) -> Option<String> {
    latest_coverage(entries, custom_type).map(|(_, id)| id)
}

pub fn earlier_coverage_marker_id(
    entries: &[Value],
    first: Option<&str>,
    second: Option<&str>,
) -> Option<String> {
    let (first, second) = match (first, second) {
        (None, s) => return s.map(str::to_owned),
        (f, None) => return f.map(str::to_owned),
        (Some(f), Some(s)) => (f, s),
    };
    let indexes = index_by_id(entries);
    match (indexes.get(first), indexes.get(second)) {
        (None, None) => None,
        (None, Some(_)) => Some(second.to_owned()),
        (Some(_), None) => Some(first.to_owned()),
        (Some(a), Some(b)) => Some(if a <= b { first } else { second }.to_owned()),
    }
}

pub fn raw_tokens_after_index(entries: &[Value], index: i64) -> u64 {
    let start = (index + 1).max(0) as usize;
    entries
        .iter()
        .skip(start)
        .filter(|e| is_source_entry(e))
        .map(estimate_entry_tokens)
        .sum()
}

pub fn raw_tokens_since_coverage(entries: &[Value], custom_type: &str) -> u64 {
    raw_tokens_after_index(entries, latest_coverage_index(entries, custom_type))
}

pub fn raw_tokens_since_observation_coverage(entries: &[Value]) -> u64 {
    raw_tokens_since_coverage(entries, OM_OBSERVATIONS_RECORDED)
}

pub fn raw_tokens_since_reflection_coverage(entries: &[Value]) -> u64 {
    raw_tokens_since_coverage(entries, OM_REFLECTIONS_RECORDED)
}

pub fn raw_tokens_since_drop_coverage(entries: &[Value]) -> u64 {
    raw_tokens_since_coverage(entries, OM_OBSERVATIONS_DROPPED)
}

pub fn find_last_compaction_index(entries: &[Value]) -> i64 {
    entries
        .iter()
        .rposition(|e| entry_type(e) == "compaction")
        .map_or(-1, |i| i as i64)
}

pub fn context_tokens_from_usage(usage: &Value) -> Option<u64> {
    let field = |key: &str| usage.get(key).and_then(Value::as_f64);
    if let Some(total) = field("totalTokens").filter(|t| t.is_finite() && *t > 0.0) {
        return Some(total as u64);
    }
    let parts: Option<Vec<f64>> = ["input", "output", "cacheRead", "cacheWrite"]
        .iter()
        .map(|k| field(k))
        .collect();
    let sum: f64 = parts?.iter().sum();
    (sum > 0.0).then_some(sum as u64)
}

fn valid_assistant_context_tokens(entry: &Value) -> Option<u64> {
    if entry_type(entry) != "message" {
        return None;
    }
    let message = entry.get("message")?;
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let stop = message.get("stopReason").and_then(Value::as_str);
    if matches!(stop, Some("aborted") | Some("error")) {
        return None;
    }
    context_tokens_from_usage(message.get("usage")?)
}

/// Usage of the first valid assistant after the compaction; the compaction's own usage is pre-cut.
pub fn real_context_tokens_after_compaction(entries: &[Value], compaction: usize) -> Option<u64> {
    entries
        .iter()
        .skip(compaction + 1)
        .find_map(valid_assistant_context_tokens)
}

pub fn real_context_tokens_at_coverage(entries: &[Value], coverage: usize) -> Option<u64> {
    entries
        .iter()
        .take(coverage + 1)
        .rev()
        .find_map(valid_assistant_context_tokens)
}

/// Provider-reported growth since the latest anchor; `None` means fall back to the raw estimate.
pub fn real_tokens_since_anchor(
    entries: &[Value],
    custom_type: Option<&str>,
    current: u64,
) -> Option<u64> {
    let coverage = custom_type.map_or(-1, |t| latest_coverage_index(entries, t));
    let compaction = find_last_compaction_index(entries);
    let baseline = if compaction > coverage {
        real_context_tokens_after_compaction(entries, compaction as usize)?
    } else if coverage >= 0 {
        real_context_tokens_at_coverage(entries, coverage as usize)?
    } else {
        return Some(current);
    };
    current.checked_sub(baseline)
}

pub fn raw_tokens_since_last_compaction(entries: &[Value]) -> u64 {
    let compaction = find_last_compaction_index(entries);
    if compaction == -1 {
        return raw_tokens_after_index(entries, -1);
    }
    let first_kept = entries[compaction as usize]
        .get("firstKeptEntryId")
        .and_then(Value::as_str);
    match index_of(entries, first_kept) {
        None => raw_tokens_after_index(entries, compaction),
        Some(kept) => raw_tokens_after_index(entries, kept as i64 - 1),
    }
}
