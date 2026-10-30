use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const OM_OBSERVATIONS_RECORDED: &str = "om.observations.recorded";
pub const OM_REFLECTIONS_RECORDED: &str = "om.reflections.recorded";
pub const OM_OBSERVATIONS_DROPPED: &str = "om.observations.dropped";
pub const OM_FOLDED: &str = "om.folded";

pub const RELEVANCE_VALUES: [&str; 4] = ["low", "medium", "high", "critical"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub id: String,
    pub content: String,
    pub timestamp: String,
    pub relevance: String,
    pub source_entry_ids: Vec<String>,
    pub token_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reflection {
    pub id: String,
    pub content: String,
    pub supporting_observation_ids: Vec<String>,
    pub token_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationsRecorded {
    pub observations: Vec<Observation>,
    pub covers_up_to_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReflectionsRecorded {
    pub reflections: Vec<Reflection>,
    pub covers_up_to_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationsDropped {
    pub observation_ids: Vec<String>,
    pub covers_up_to_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDetails {
    #[serde(rename = "type")]
    pub kind: String,
    pub version: u64,
    pub full_fold: bool,
    pub observations: Vec<Observation>,
    pub reflections: Vec<Reflection>,
}

pub fn is_relevance(value: &str) -> bool {
    RELEVANCE_VALUES.contains(&value)
}

pub fn is_memory_id(value: &str) -> bool {
    value.len() == 12
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn non_empty_ids(ids: &[String]) -> bool {
    !ids.is_empty() && ids.iter().all(|id| !id.is_empty())
}

pub fn is_valid_observation(o: &Observation) -> bool {
    is_memory_id(&o.id)
        && !o.content.is_empty()
        && !o.timestamp.is_empty()
        && is_relevance(&o.relevance)
        && non_empty_ids(&o.source_entry_ids)
}

pub fn is_valid_reflection(r: &Reflection) -> bool {
    is_memory_id(&r.id)
        && !r.content.is_empty()
        && !r.content.contains(['\r', '\n'])
        && non_empty_ids(&r.supporting_observation_ids)
}

pub fn entry_type(entry: &Value) -> &str {
    entry.get("type").and_then(Value::as_str).unwrap_or("")
}

pub fn entry_id(entry: &Value) -> &str {
    entry.get("id").and_then(Value::as_str).unwrap_or("")
}

fn custom_data<'a>(entry: &'a Value, custom_type: &str) -> Option<&'a Value> {
    let custom = entry_type(entry) == "custom"
        && entry.get("customType").and_then(Value::as_str) == Some(custom_type);
    custom.then(|| entry.get("data")).flatten()
}

pub fn observations_recorded(entry: &Value) -> Option<ObservationsRecorded> {
    let data: ObservationsRecorded =
        serde_json::from_value(custom_data(entry, OM_OBSERVATIONS_RECORDED)?.clone()).ok()?;
    let valid = !data.observations.is_empty()
        && data.observations.iter().all(is_valid_observation)
        && !data.covers_up_to_id.is_empty();
    valid.then_some(data)
}

pub fn reflections_recorded(entry: &Value) -> Option<ReflectionsRecorded> {
    let data: ReflectionsRecorded =
        serde_json::from_value(custom_data(entry, OM_REFLECTIONS_RECORDED)?.clone()).ok()?;
    let valid = !data.reflections.is_empty()
        && data.reflections.iter().all(is_valid_reflection)
        && !data.covers_up_to_id.is_empty();
    valid.then_some(data)
}

pub fn observations_dropped(entry: &Value) -> Option<ObservationsDropped> {
    let data: ObservationsDropped =
        serde_json::from_value(custom_data(entry, OM_OBSERVATIONS_DROPPED)?.clone()).ok()?;
    let valid = non_empty_ids(&data.observation_ids) && !data.covers_up_to_id.is_empty();
    valid.then_some(data)
}

pub fn memory_details(value: &Value) -> Option<MemoryDetails> {
    let details: MemoryDetails = serde_json::from_value(value.clone()).ok()?;
    let valid = details.kind == OM_FOLDED
        && details.version == 1
        && details.observations.iter().all(is_valid_observation)
        && details.reflections.iter().all(is_valid_reflection);
    valid.then_some(details)
}

/// Coverage marker of a memory entry: shape checks only, as in upstream `progress.ts`.
pub fn coverage_marker<'a>(entry: &'a Value, custom_type: &str) -> Option<&'a str> {
    let data = custom_data(entry, custom_type)?;
    let covers = data.get("coversUpToId")?.as_str()?;
    let list_key = match custom_type {
        OM_OBSERVATIONS_RECORDED => "observations",
        OM_REFLECTIONS_RECORDED => "reflections",
        _ => "observationIds",
    };
    let non_empty = data
        .get(list_key)
        .and_then(Value::as_array)
        .is_some_and(|list| !list.is_empty());
    non_empty.then_some(covers)
}
