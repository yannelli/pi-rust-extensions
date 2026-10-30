use std::collections::HashSet;

use serde_json::Value;

use super::types::{
    Observation, Reflection, entry_id, observations_dropped, observations_recorded,
    reflections_recorded,
};

/// The ledger folded from the branch root through a boundary entry.
#[derive(Debug, Clone, Default)]
pub struct FoldedLedger {
    pub observations: Vec<Observation>,
    pub active_observations: Vec<Observation>,
    pub dropped_observation_ids: HashSet<String>,
    pub reflections: Vec<Reflection>,
}

/// First valid record wins per id; drops are tombstones kept even for unknown ids.
pub fn fold_ledger(entries: &[Value], up_to_entry_id: Option<&str>) -> FoldedLedger {
    let end = up_to_entry_id
        .and_then(|id| entries.iter().position(|e| entry_id(e) == id))
        .map_or(entries.len(), |i| i + 1);
    let mut observations: Vec<Observation> = Vec::new();
    let mut reflections: Vec<Reflection> = Vec::new();
    let mut seen_obs = HashSet::new();
    let mut seen_ref = HashSet::new();
    let mut dropped = HashSet::new();

    for entry in &entries[..end] {
        if let Some(data) = observations_recorded(entry) {
            for o in data.observations {
                if seen_obs.insert(o.id.clone()) {
                    observations.push(o);
                }
            }
        } else if let Some(data) = reflections_recorded(entry) {
            for r in data.reflections {
                if seen_ref.insert(r.id.clone()) {
                    reflections.push(r);
                }
            }
        } else if let Some(data) = observations_dropped(entry) {
            dropped.extend(data.observation_ids);
        }
    }

    let active_observations = observations
        .iter()
        .filter(|o| !dropped.contains(&o.id))
        .cloned()
        .collect();
    FoldedLedger {
        observations,
        active_observations,
        dropped_observation_ids: dropped,
        reflections,
    }
}
