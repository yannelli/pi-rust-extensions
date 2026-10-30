#![allow(dead_code)]

use pi_ext_observational_memory::ledger::{Observation, Reflection};
use pi_ext_observational_memory::tokens::{
    estimate_string_tokens, hash_id, observation_line_token_count,
};
use serde_json::{Value, json};

pub const TS_MS: i64 = 1_793_000_000_000;

pub fn user(id: &str, text: &str) -> Value {
    json!({ "type": "message", "id": id, "timestamp": "2026-10-29T10:00:00.000Z",
        "message": { "role": "user", "content": text, "timestamp": TS_MS } })
}

pub fn assistant(id: &str, text: &str, total_tokens: Option<u64>) -> Value {
    let mut message = json!({ "role": "assistant", "content": [{ "type": "text", "text": text }],
        "timestamp": TS_MS, "stopReason": "stop" });
    if let Some(total) = total_tokens {
        message["usage"] = json!({ "input": total, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": total });
    }
    json!({ "type": "message", "id": id, "timestamp": "2026-10-29T10:00:00.000Z", "message": message })
}

pub fn observation(content: &str, relevance: &str, sources: &[&str]) -> Observation {
    let mut o = Observation {
        id: hash_id(content),
        content: content.to_owned(),
        timestamp: "2026-10-29 10:00".to_owned(),
        relevance: relevance.to_owned(),
        source_entry_ids: sources.iter().map(|s| (*s).to_owned()).collect(),
        token_count: 0,
    };
    o.token_count = observation_line_token_count(&o);
    o
}

pub fn reflection(content: &str, supporting: &[&str]) -> Reflection {
    Reflection {
        id: hash_id(content),
        content: content.to_owned(),
        supporting_observation_ids: supporting.iter().map(|s| (*s).to_owned()).collect(),
        token_count: estimate_string_tokens(content),
    }
}

pub fn custom(id: &str, custom_type: &str, data: Value) -> Value {
    json!({ "type": "custom", "id": id, "customType": custom_type, "data": data })
}

pub fn observations_entry(id: &str, observations: &[Observation], covers: &str) -> Value {
    custom(
        id,
        "om.observations.recorded",
        json!({ "observations": observations, "coversUpToId": covers }),
    )
}

pub fn reflections_entry(id: &str, reflections: &[Reflection], covers: &str) -> Value {
    custom(
        id,
        "om.reflections.recorded",
        json!({ "reflections": reflections, "coversUpToId": covers }),
    )
}

pub fn dropped_entry(id: &str, ids: &[&str], covers: &str) -> Value {
    custom(
        id,
        "om.observations.dropped",
        json!({ "observationIds": ids, "coversUpToId": covers }),
    )
}

pub fn compaction(id: &str, first_kept: &str, details: Value) -> Value {
    json!({ "type": "compaction", "id": id, "summary": "s", "firstKeptEntryId": first_kept,
        "tokensBefore": 100, "details": details })
}

/// `n` user messages of `chars` characters each, ids `u0..`.
pub fn conversation(n: usize, chars: usize) -> Vec<Value> {
    (0..n)
        .map(|i| user(&format!("u{i}"), &"x".repeat(chars)))
        .collect()
}
