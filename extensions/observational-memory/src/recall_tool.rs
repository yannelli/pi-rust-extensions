use pi_ext_sdk::prelude::*;
use serde::Deserialize;

use crate::ledger::{RecallFound, RecalledObservation, is_memory_id, recall_memory_sources};
use crate::serialize::render_recall_source_entries;

pub const RECALL_TOOL_NAME: &str = "recall";

#[derive(Deserialize)]
pub struct RecallArgs {
    pub id: String,
}

pub fn spec() -> ToolSpec {
    let mut spec = ToolSpec::new(
        RECALL_TOOL_NAME,
        "Recover exact evidence and source context behind a compacted observational-memory observation or reflection id on the current branch. Use when compressed memory is important and original source context is needed before acting.",
    )
    .label("Recall memory evidence")
    .prompt_snippet("Use recall(<id>) to recover exact source context behind compacted memory observations/reflections when precision matters.")
    .parameters(json!({
        "type": "object",
        "properties": { "id": {
            "type": "string",
            "pattern": "^[a-f0-9]{12}$",
            "description": "12-character lowercase hex observation or reflection id shown in compacted memory, /om:view, or a previous recall result. Must be a specific id; this tool does not search by topic."
        } },
        "required": ["id"]
    }));
    for guideline in [
        "Use recall before making an important decision that depends on a compacted observation or reflection whose details are unclear.",
        "Use recall when you need exact wording, rationale, file paths, commands, errors, commits, user constraints, or provenance behind a remembered claim.",
        "Use recall when a broad reflection is relevant but you need its supporting observations or raw sources to continue safely.",
        "Use recall when the user asks why you believe something, what supports a memory, or what was decided earlier.",
        "Do not use recall as semantic search or transcript browsing; you must already have a specific 12-character memory id.",
        "Do not recall every id preemptively. Recall only when exact source context will materially improve the next action.",
    ] {
        spec = spec.guideline(guideline);
    }
    spec
}

fn empty_details(status: &str, id: &str, message: &str) -> Value {
    json!({
        "status": status, "memoryId": id, "observationId": id, "collision": false, "partial": false,
        "reflections": [], "observations": [], "sourceEntries": [], "unavailableSupportingObservations": [],
        "missingSourceEntryIds": [], "nonSourceEntryIds": [], "message": message,
    })
}

fn unavailable(o: &RecalledObservation) -> bool {
    !o.missing_source_entry_ids.is_empty() || !o.non_source_entry_ids.is_empty()
}

fn status(found: &RecallFound) -> &'static str {
    let observation_only =
        found.reflections.is_empty() && found.missing_supporting_observation_ids.is_empty();
    if found.partial {
        "partial"
    } else if observation_only && found.observations.iter().any(unavailable) {
        "source_unavailable"
    } else if observation_only
        && !found.observations.is_empty()
        && found
            .observations
            .iter()
            .all(|o| o.source_entries.is_empty())
    {
        "no_source"
    } else {
        "ok"
    }
}

fn observation_text(o: &RecalledObservation) -> String {
    let dropped = if o.dropped { " [dropped]" } else { "" };
    let ob = &o.observation;
    format!(
        "[{}]{dropped} {} [{}] {}",
        ob.id, ob.timestamp, ob.relevance, ob.content
    )
}

fn source_unavailable_message(o: &RecalledObservation) -> String {
    let mut text = format!(
        "Observation {} has source entries associated, but some are unavailable on the current branch or are not source-renderable.",
        o.observation.id
    );
    if !o.missing_source_entry_ids.is_empty() {
        text += &format!(" missing: {}", o.missing_source_entry_ids.join(", "));
    }
    if !o.non_source_entry_ids.is_empty() {
        text += &format!(" non-source: {}", o.non_source_entry_ids.join(", "));
    }
    text
}

fn observation_only_text(found: &RecallFound) -> String {
    let mut sections = Vec::new();
    if found.collision {
        sections.push(format!("Memory id {} matched multiple observations; returning all matching source results from the current branch.", found.memory_id));
    }
    for o in found
        .observations
        .iter()
        .filter(|o| o.observation.id == found.memory_id)
    {
        let id = &o.observation.id;
        if o.dropped {
            sections.push(format!(
                "Observation {id} is dropped from active memory but remains recallable."
            ));
        }
        if unavailable(o) {
            sections.push(source_unavailable_message(o));
        } else if o.source_entries.is_empty() {
            sections.push(format!(
                "Observation {id} has no source entries associated with it."
            ));
        } else {
            let text = render_recall_source_entries(&o.source_entries);
            sections.push(if text.trim().is_empty() {
                format!("Observation {id} has source entries associated, but they rendered no text content.")
            } else {
                text
            });
        }
    }
    sections.join("\n\n")
}

fn memory_text(found: &RecallFound) -> String {
    let mut sections = Vec::new();
    if found.collision {
        sections.push(format!("Memory id {} matched multiple observations/reflections; returning all available evidence from the current branch.", found.memory_id));
    }
    if !found.reflections.is_empty() {
        let lines: Vec<String> = found
            .reflections
            .iter()
            .map(|r| format!("[{}] {}", r.id, r.content))
            .collect();
        sections.push(format!("Reflections:\n{}", lines.join("\n")));
    }
    if !found.observations.is_empty() {
        let lines: Vec<String> = found.observations.iter().map(observation_text).collect();
        sections.push(format!("Observations:\n{}", lines.join("\n")));
    }
    if !found.missing_supporting_observation_ids.is_empty() {
        let lines: Vec<String> = found
            .missing_supporting_observation_ids
            .iter()
            .map(|id| format!("Supporting observation {id} is unavailable on the current branch."))
            .collect();
        sections.push(format!(
            "Unavailable supporting observations:\n{}",
            lines.join("\n")
        ));
    }
    let mut parts = Vec::new();
    if !found.missing_source_entry_ids.is_empty() {
        parts.push(format!(
            "missing: {}",
            found.missing_source_entry_ids.join(", ")
        ));
    }
    if !found.non_source_entry_ids.is_empty() {
        parts.push(format!(
            "non-source: {}",
            found.non_source_entry_ids.join(", ")
        ));
    }
    if !parts.is_empty() {
        sections.push(format!("Unavailable source entries: {}", parts.join("; ")));
    }
    let sources = render_recall_source_entries(&found.source_entries);
    if !sources.trim().is_empty() {
        sections.push(format!("Sources:\n{sources}"));
    }
    if sections.is_empty() {
        sections.push(format!(
            "Memory {} was found, but no source evidence rendered.",
            found.memory_id
        ));
    }
    sections.join("\n\n")
}

fn found_details(found: &RecallFound) -> Value {
    json!({
        "status": status(found), "memoryId": found.memory_id, "observationId": found.memory_id,
        "kind": found.kind, "collision": found.collision, "partial": found.partial,
        "reflections": found.reflections,
        "observations": found.observations.iter().map(|o| json!({
            "observation": o.observation, "status": if o.dropped { "dropped" } else { "active" },
            "observationEntryId": o.observation_entry_id, "observationRecordIndex": o.record_index,
        })).collect::<Vec<_>>(),
        "sourceEntryIds": found.source_entries.iter().map(|e| e["id"].clone()).collect::<Vec<_>>(),
        "unavailableSupportingObservations": found.missing_supporting_observation_ids,
        "missingSourceEntryIds": found.missing_source_entry_ids,
        "nonSourceEntryIds": found.non_source_entry_ids,
    })
}

/// The tool result for `id` against the branch entries.
pub fn recall(entries: &[Value], id: &str) -> ToolResult {
    if !is_memory_id(id) {
        let message = format!("Memory id must be 12 lowercase hex characters. Received: {id}");
        return ToolResult::text(&message).details(empty_details("invalid_id", id, &message));
    }
    let Some(found) = recall_memory_sources(entries, id) else {
        let message =
            format!("No observation or reflection with id {id} was found on the current branch.");
        return ToolResult::text(&message).details(empty_details("not_found", id, &message));
    };
    let text = if found.kind == "observation" {
        observation_only_text(&found)
    } else {
        memory_text(&found)
    };
    ToolResult::text(text).details(found_details(&found))
}

pub async fn execute(args: RecallArgs, ctx: ToolCtx) -> Result<ToolResult> {
    if !is_memory_id(&args.id) {
        return Ok(recall(&[], &args.id));
    }
    let entries = ctx.host().branch(None).await?;
    Ok(recall(&entries, &args.id))
}
