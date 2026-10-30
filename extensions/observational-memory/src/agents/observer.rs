use std::collections::HashMap;

use serde_json::Value;

use super::completion::{CallError, Completer, array_field, call_json, join_or_empty};
use crate::ledger::{Observation, is_relevance};
use crate::prompts::observer::OBSERVER_SYSTEM;
use crate::serialize::{now_timestamp, truncate_record_content};
use crate::tokens::{hash_id, observation_line_token_count};

pub struct ObserverInput<'a> {
    pub prior_reflections: &'a [String],
    pub prior_observations: &'a [String],
    pub chunk: &'a str,
    pub allowed_source_entry_ids: &'a [String],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub timestamp: String,
    pub content: String,
    pub relevance: String,
    pub source_entry_ids: Vec<String>,
}

pub fn is_observation_timestamp(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() == 16
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b' ',
            13 => *c == b':',
            _ => c.is_ascii_digit(),
        })
}

fn proposal(item: &Value) -> Option<Proposal> {
    let str_of = |k: &str| item.get(k)?.as_str().map(str::to_owned);
    let ids: Vec<String> = item
        .get("sourceEntryIds")?
        .as_array()?
        .iter()
        .map(|v| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned))
        .collect::<Option<_>>()?;
    let p = Proposal {
        timestamp: str_of("timestamp").filter(|t| is_observation_timestamp(t))?,
        content: str_of("content").filter(|c| !c.is_empty())?,
        relevance: str_of("relevance").filter(|r| is_relevance(r))?,
        source_entry_ids: ids,
    };
    (!p.source_entry_ids.is_empty()).then_some(p)
}

/// Schema-valid proposals; a non-empty list with no valid item counts as a bad reply.
pub fn parse_reply(value: &Value) -> Result<Vec<Proposal>, String> {
    let items = array_field(value, "observations")?;
    let proposals: Vec<Proposal> = items.iter().filter_map(proposal).collect();
    if !items.is_empty() && proposals.is_empty() {
        return Err("no observation matched the schema".to_owned());
    }
    Ok(proposals)
}

/// Allowed ids in chunk order, or `None` when any id is outside the chunk.
pub fn normalize_source_entry_ids(ids: &[String], allowed: &[String]) -> Option<Vec<String>> {
    let order: HashMap<&str, usize> = allowed
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    let mut seen: Vec<&String> = Vec::new();
    for id in ids {
        if !order.contains_key(id.as_str()) {
            return None;
        }
        if !seen.contains(&id) {
            seen.push(id);
        }
    }
    seen.sort_by_key(|id| order[id.as_str()]);
    (!seen.is_empty()).then(|| seen.into_iter().cloned().collect())
}

/// Validated, deduplicated observations from the proposals.
pub fn accept(proposals: Vec<Proposal>, allowed: &[String]) -> Vec<Observation> {
    let mut accepted: Vec<Observation> = Vec::new();
    for p in proposals {
        let Some(source_entry_ids) = normalize_source_entry_ids(&p.source_entry_ids, allowed)
        else {
            continue;
        };
        let content = truncate_record_content(&p.content);
        let id = hash_id(&content);
        if accepted.iter().any(|o| o.id == id) {
            continue;
        }
        let mut observation = Observation {
            id,
            content,
            timestamp: p.timestamp,
            relevance: p.relevance,
            source_entry_ids,
            token_count: 0,
        };
        observation.token_count = observation_line_token_count(&observation);
        accepted.push(observation);
    }
    accepted
}

pub fn user_prompt(input: &ObserverInput, now: &str) -> String {
    format!(
        "Current local time: {now}\n\nCURRENT REFLECTIONS:\n{}\n\nCURRENT OBSERVATIONS:\n{}\n\nCompress the following new conversation chunk into observations and return them as the JSON object described in the output format. Do not restate facts already present in current reflections or current observations. Prefer inline conversation timestamps when assigning times; fall back to the current local time above only if no message timestamp applies.\n\nNEW CONVERSATION CHUNK:\n{}",
        join_or_empty(input.prior_reflections),
        join_or_empty(input.prior_observations),
        input.chunk.trim()
    )
}

/// New observations; an empty list is the observer's deliberate "nothing new" verdict.
pub async fn run_observer<C: Completer>(
    completer: &C,
    input: &ObserverInput<'_>,
) -> Result<Vec<Observation>, CallError> {
    if input.chunk.trim().is_empty() {
        return Ok(Vec::new());
    }
    let prompt = user_prompt(input, &now_timestamp());
    let proposals = call_json(completer, OBSERVER_SYSTEM, &prompt, parse_reply).await?;
    Ok(accept(proposals, input.allowed_source_entry_ids))
}
