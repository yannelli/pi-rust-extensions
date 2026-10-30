use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::progress::index_by_id;
use super::types::{
    MemoryDetails, OM_FOLDED, Observation, Reflection, entry_type, memory_details,
    observations_dropped, observations_recorded, reflections_recorded,
};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Projection {
    pub observations: Vec<Observation>,
    pub reflections: Vec<Reflection>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectionDiff {
    pub observations_only_in_full: Vec<Observation>,
    pub reflections_only_in_full: Vec<Reflection>,
    pub dropped_only_in_full: Vec<Observation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompactionProjection {
    pub full_fold: bool,
    pub projection: Projection,
    pub details: MemoryDetails,
}

#[derive(Clone, Copy)]
enum Boundary<'a> {
    Entry(&'a str),
    Tip,
    None,
}

fn boundary_index(entries: &[Value], indexes: &HashMap<&str, usize>, b: Boundary) -> i64 {
    match b {
        Boundary::Tip => entries.len() as i64 - 1,
        Boundary::None => -1,
        Boundary::Entry(id) => indexes.get(id).map_or(-1, |i| *i as i64),
    }
}

fn covered(indexes: &HashMap<&str, usize>, covers: &str, boundary: i64) -> bool {
    indexes
        .get(covers)
        .is_some_and(|i| boundary >= 0 && (*i as i64) <= boundary)
}

fn fold_projection(
    entries: &[Value],
    obs: Boundary,
    refl: Boundary,
    drops: Boundary,
) -> Projection {
    let indexes = index_by_id(entries);
    let obs_b = boundary_index(entries, &indexes, obs);
    let refl_b = boundary_index(entries, &indexes, refl);
    let drops_b = boundary_index(entries, &indexes, drops);
    let mut observations: Vec<Observation> = Vec::new();
    let mut reflections: Vec<Reflection> = Vec::new();
    let mut seen_obs = HashSet::new();
    let mut seen_ref = HashSet::new();
    let mut dropped = HashSet::new();

    for entry in entries {
        if let Some(data) = observations_recorded(entry) {
            if covered(&indexes, &data.covers_up_to_id, obs_b) {
                for o in data.observations {
                    if seen_obs.insert(o.id.clone()) {
                        observations.push(o);
                    }
                }
            }
        } else if let Some(data) = reflections_recorded(entry) {
            if covered(&indexes, &data.covers_up_to_id, refl_b) {
                for r in data.reflections {
                    if seen_ref.insert(r.id.clone()) {
                        reflections.push(r);
                    }
                }
            }
        } else if let Some(data) = observations_dropped(entry) {
            if covered(&indexes, &data.covers_up_to_id, drops_b) {
                dropped.extend(data.observation_ids);
            }
        }
    }
    observations.retain(|o| !dropped.contains(&o.id));
    Projection {
        observations,
        reflections,
    }
}

fn latest_compaction_details(entries: &[Value]) -> Option<MemoryDetails> {
    entries
        .iter()
        .rev()
        .filter(|e| entry_type(e) == "compaction")
        .find_map(|e| e.get("details").and_then(memory_details))
}

pub fn full_projection(entries: &[Value], up_to_entry_id: Option<&str>) -> Projection {
    let b = up_to_entry_id.map_or(Boundary::Tip, Boundary::Entry);
    fold_projection(entries, b, b, b)
}

pub fn visible_projection(entries: &[Value], up_to_entry_id: Option<&str>) -> Projection {
    match up_to_entry_id {
        None => latest_compaction_details(entries)
            .map(|d| Projection {
                observations: d.observations,
                reflections: d.reflections,
            })
            .unwrap_or_default(),
        Some(id) => build_compaction_projection(entries, id, u64::MAX).projection,
    }
}

pub fn latest_full_fold_boundary_id(entries: &[Value]) -> Option<String> {
    let indexes = index_by_id(entries);
    entries
        .iter()
        .rev()
        .filter(|e| entry_type(e) == "compaction")
        .find_map(|e| {
            let details = e.get("details").and_then(memory_details)?;
            let kept = e.get("firstKeptEntryId").and_then(Value::as_str)?;
            (details.full_fold && indexes.contains_key(kept)).then(|| kept.to_owned())
        })
}

pub fn build_compaction_projection(
    entries: &[Value],
    first_kept_entry_id: &str,
    pool_max_tokens: u64,
) -> CompactionProjection {
    let full_fold_boundary = latest_full_fold_boundary_id(entries);
    let maintenance = full_fold_boundary
        .as_deref()
        .map_or(Boundary::None, Boundary::Entry);
    let normal = fold_projection(
        entries,
        Boundary::Entry(first_kept_entry_id),
        maintenance,
        maintenance,
    );
    let observation_tokens: u64 = normal.observations.iter().map(|o| o.token_count).sum();
    let full_fold = observation_tokens >= pool_max_tokens;
    let projection = if full_fold {
        full_projection(entries, Some(first_kept_entry_id))
    } else {
        normal
    };
    let details = MemoryDetails {
        kind: OM_FOLDED.to_owned(),
        version: 1,
        full_fold,
        observations: projection.observations.clone(),
        reflections: projection.reflections.clone(),
    };
    CompactionProjection {
        full_fold,
        projection,
        details,
    }
}

pub fn diff_projection(visible: &Projection, full: &Projection) -> ProjectionDiff {
    let visible_obs: HashSet<&str> = visible.observations.iter().map(|o| o.id.as_str()).collect();
    let full_obs: HashSet<&str> = full.observations.iter().map(|o| o.id.as_str()).collect();
    let visible_ref: HashSet<&str> = visible.reflections.iter().map(|r| r.id.as_str()).collect();
    ProjectionDiff {
        observations_only_in_full: full
            .observations
            .iter()
            .filter(|o| !visible_obs.contains(o.id.as_str()))
            .cloned()
            .collect(),
        reflections_only_in_full: full
            .reflections
            .iter()
            .filter(|r| !visible_ref.contains(r.id.as_str()))
            .cloned()
            .collect(),
        dropped_only_in_full: visible
            .observations
            .iter()
            .filter(|o| !full_obs.contains(o.id.as_str()))
            .cloned()
            .collect(),
    }
}
