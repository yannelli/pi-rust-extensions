use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::completion::{CallError, Completer, array_field, call_json, join_or_empty};
use super::coverage::{coverage_map, observation_coverage_line, tier_of};
use crate::ledger::{Observation, Reflection, reflection_to_summary_line};
use crate::prompts::reflector::REFLECTOR_SYSTEM;
use crate::serialize::truncate_record_content;
use crate::tokens::{estimate_string_tokens, hash_id};

#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub content: String,
    pub supporting_observation_ids: Vec<String>,
}

fn proposal(item: &Value) -> Option<Proposal> {
    let ids: Vec<String> = item
        .get("supportingObservationIds")?
        .as_array()?
        .iter()
        .map(|v| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned))
        .collect::<Option<_>>()?;
    let content = item.get("content")?.as_str().filter(|c| !c.is_empty())?;
    (!ids.is_empty()).then(|| Proposal {
        content: content.to_owned(),
        supporting_observation_ids: ids,
    })
}

pub fn parse_reply(value: &Value) -> Result<Vec<Proposal>, String> {
    let items = array_field(value, "reflections")?;
    let proposals: Vec<Proposal> = items.iter().filter_map(proposal).collect();
    if !items.is_empty() && proposals.is_empty() {
        return Err("no reflection matched the schema".to_owned());
    }
    Ok(proposals)
}

pub fn normalize_supporting_ids(ids: &[String], allowed: &[String]) -> Option<Vec<String>> {
    let mut order: HashMap<&str, usize> = HashMap::new();
    for (i, id) in allowed.iter().enumerate() {
        order.entry(id.as_str()).or_insert(i);
    }
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

fn normalize_content(content: &str) -> Option<String> {
    let normalized = truncate_record_content(content.trim());
    (!normalized.is_empty() && !normalized.contains(['\r', '\n'])).then_some(normalized)
}

pub fn accept(
    proposals: Vec<Proposal>,
    existing: &[Reflection],
    observations: &[Observation],
) -> Vec<Reflection> {
    let allowed: Vec<String> = observations.iter().map(|o| o.id.clone()).collect();
    let existing_ids: HashSet<&str> = existing.iter().map(|r| r.id.as_str()).collect();
    let mut accepted: Vec<Reflection> = Vec::new();
    for p in proposals {
        let (Some(content), Some(ids)) = (
            normalize_content(&p.content),
            normalize_supporting_ids(&p.supporting_observation_ids, &allowed),
        ) else {
            continue;
        };
        let id = hash_id(&content);
        if existing_ids.contains(id.as_str()) || accepted.iter().any(|r| r.id == id) {
            continue;
        }
        accepted.push(Reflection {
            id,
            token_count: estimate_string_tokens(&content),
            content,
            supporting_observation_ids: ids,
        });
    }
    accepted
}

pub fn user_prompt(reflections: &[Reflection], observations: &[Observation]) -> String {
    let coverage = coverage_map(observations, reflections);
    let reflection_lines: Vec<String> =
        reflections.iter().map(reflection_to_summary_line).collect();
    let observation_lines: Vec<String> = observations
        .iter()
        .map(|o| observation_coverage_line(o, tier_of(o, &coverage)))
        .collect();
    format!(
        "CURRENT REFLECTIONS:\n{}\n\nCURRENT OBSERVATIONS:\n{}\n\nCrystallize any missing durable facts or patterns into new reflections. If nothing is stable enough, reply with an empty \"reflections\" array.",
        join_or_empty(&reflection_lines),
        join_or_empty(&observation_lines)
    )
}

/// New reflections; empty when nothing is stable enough or there are no observations.
pub async fn run_reflector<C: Completer>(
    completer: &C,
    reflections: &[Reflection],
    observations: &[Observation],
) -> Result<Vec<Reflection>, CallError> {
    if observations.is_empty() {
        return Ok(Vec::new());
    }
    let prompt = user_prompt(reflections, observations);
    let proposals = call_json(completer, REFLECTOR_SYSTEM, &prompt, parse_reply).await?;
    Ok(accept(proposals, reflections, observations))
}
