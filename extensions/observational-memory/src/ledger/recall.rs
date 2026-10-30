use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::progress::is_source_entry;
use super::types::{
    Observation, Reflection, entry_id, observations_dropped, observations_recorded,
    reflections_recorded,
};

#[derive(Debug, Clone, PartialEq)]
pub struct RecalledObservation {
    pub observation: Observation,
    pub observation_entry_id: String,
    pub record_index: usize,
    pub dropped: bool,
    pub source_entries: Vec<Value>,
    pub missing_source_entry_ids: Vec<String>,
    pub non_source_entry_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecallFound {
    pub memory_id: String,
    pub kind: &'static str,
    pub reflections: Vec<Reflection>,
    pub observations: Vec<RecalledObservation>,
    pub source_entries: Vec<Value>,
    pub missing_source_entry_ids: Vec<String>,
    pub non_source_entry_ids: Vec<String>,
    pub missing_supporting_observation_ids: Vec<String>,
    pub collision: bool,
    pub partial: bool,
}

struct Indexed<T> {
    record: T,
    entry_id: String,
    record_index: usize,
}

fn unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter(|v| seen.insert(v.clone()))
        .collect()
}

fn resolve(
    entries: &[Value],
    indexed: &Indexed<Observation>,
    dropped: bool,
) -> RecalledObservation {
    let by_id: HashMap<&str, &Value> = entries.iter().map(|e| (entry_id(e), e)).collect();
    let mut recalled = RecalledObservation {
        observation: indexed.record.clone(),
        observation_entry_id: indexed.entry_id.clone(),
        record_index: indexed.record_index,
        dropped,
        source_entries: Vec::new(),
        missing_source_entry_ids: Vec::new(),
        non_source_entry_ids: Vec::new(),
    };
    for id in unique(indexed.record.source_entry_ids.clone()) {
        match by_id.get(id.as_str()) {
            None => recalled.missing_source_entry_ids.push(id),
            Some(e) if !is_source_entry(e) => recalled.non_source_entry_ids.push(id),
            Some(e) => recalled.source_entries.push((*e).clone()),
        }
    }
    recalled
}

/// Finds `memory_id` among observations and reflections on the branch; `None` when absent.
pub fn recall_memory_sources(entries: &[Value], memory_id: &str) -> Option<RecallFound> {
    let mut observations = Vec::new();
    let mut reflections = Vec::new();
    let mut dropped_ids = HashSet::new();
    for entry in entries {
        let id = entry_id(entry).to_owned();
        if let Some(data) = observations_recorded(entry) {
            for (i, o) in data.observations.into_iter().enumerate() {
                observations.push(Indexed {
                    record: o,
                    entry_id: id.clone(),
                    record_index: i,
                });
            }
        } else if let Some(data) = reflections_recorded(entry) {
            for (i, r) in data.reflections.into_iter().enumerate() {
                reflections.push(Indexed {
                    record: r,
                    entry_id: id.clone(),
                    record_index: i,
                });
            }
        } else if let Some(data) = observations_dropped(entry) {
            dropped_ids.extend(data.observation_ids);
        }
    }
    let direct: Vec<&Indexed<Observation>> = observations
        .iter()
        .filter(|o| o.record.id == memory_id)
        .collect();
    let reflection_matches: Vec<&Indexed<Reflection>> = reflections
        .iter()
        .filter(|r| r.record.id == memory_id)
        .collect();
    if direct.is_empty() && reflection_matches.is_empty() {
        return None;
    }

    let mut first_by_id: HashMap<&str, &Indexed<Observation>> = HashMap::new();
    for o in &observations {
        first_by_id.entry(o.record.id.as_str()).or_insert(o);
    }
    let mut recalled: Vec<RecalledObservation> = Vec::new();
    let mut keys = HashSet::new();
    let mut add = |o: &Indexed<Observation>| {
        if keys.insert((o.entry_id.clone(), o.record_index)) {
            recalled.push(resolve(entries, o, dropped_ids.contains(&o.record.id)));
        }
    };
    for o in &direct {
        add(o);
    }
    let mut missing_support = Vec::new();
    for r in &reflection_matches {
        for id in unique(r.record.supporting_observation_ids.clone()) {
            match first_by_id.get(id.as_str()) {
                Some(o) => add(o),
                None => missing_support.push(id),
            }
        }
    }

    let mut seen_sources = HashSet::new();
    let source_entries: Vec<Value> = recalled
        .iter()
        .flat_map(|o| o.source_entries.iter())
        .filter(|e| seen_sources.insert(entry_id(e).to_owned()))
        .cloned()
        .collect();
    let missing = unique(
        recalled
            .iter()
            .flat_map(|o| o.missing_source_entry_ids.clone()),
    );
    let non_source = unique(recalled.iter().flat_map(|o| o.non_source_entry_ids.clone()));
    let missing_support = unique(missing_support);
    let kind = match (direct.is_empty(), reflection_matches.is_empty()) {
        (false, false) => "mixed",
        (true, false) => "reflection",
        _ => "observation",
    };
    Some(RecallFound {
        memory_id: memory_id.to_owned(),
        kind,
        collision: direct.len() + reflection_matches.len() > 1,
        partial: !missing.is_empty() || !non_source.is_empty() || !missing_support.is_empty(),
        reflections: reflection_matches
            .iter()
            .map(|r| r.record.clone())
            .collect(),
        observations: recalled,
        source_entries,
        missing_source_entry_ids: missing,
        non_source_entry_ids: non_source,
        missing_supporting_observation_ids: missing_support,
    })
}
