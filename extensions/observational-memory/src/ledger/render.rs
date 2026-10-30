use super::types::{Observation, Reflection};
use crate::tokens::observation_line;

const CONTEXT_USAGE_INSTRUCTIONS: &str = "These are condensed memories from earlier in this session.

- Reflections: stable, long-lived facts about the user, project, decisions, and constraints. New reflection lines may include ids in brackets.
- Observations: timestamped events from the conversation history, in chronological order. Observation lines include ids in brackets.

Treat these as past records. When entries conflict, the most recent observation reflects the latest known state. Work that prior observations describe as completed should not be redone unless the user explicitly asks to revisit it.

When exact source context is needed for precision or traceability, use the recall tool with the relevant observation or reflection id. This is especially useful when a reflection materially affects a decision or is too compressed to continue confidently. Do not use recall as broad search or inject raw source unless it is needed.";

pub fn observation_to_summary_line(o: &Observation) -> String {
    observation_line(&o.id, &o.timestamp, &o.relevance, &o.content)
}

pub fn reflection_to_summary_line(r: &Reflection) -> String {
    format!("[{}] {}", r.id, r.content)
}

/// The compaction summary; empty when there is no memory.
pub fn render_summary(reflections: &[Reflection], observations: &[Observation]) -> String {
    if reflections.is_empty() && observations.is_empty() {
        return String::new();
    }
    let mut parts = vec![CONTEXT_USAGE_INSTRUCTIONS.to_owned()];
    if !reflections.is_empty() {
        let lines: Vec<String> = reflections.iter().map(reflection_to_summary_line).collect();
        parts.push(format!("## Reflections\n{}", lines.join("\n")));
    }
    if !observations.is_empty() {
        let lines: Vec<String> = observations
            .iter()
            .map(observation_to_summary_line)
            .collect();
        parts.push(format!("## Observations\n{}", lines.join("\n")));
    }
    parts.join("\n\n")
}
