use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::ledger::types::{Observation, entry_type};

const ESTIMATED_IMAGE_CHARS: usize = 4800;

/// 12 lowercase hex characters of the SHA-256 of `content`.
pub fn hash_id(content: &str) -> String {
    let digest = Sha256::digest(content.as_bytes());
    digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()[..12]
        .to_owned()
}

/// UTF-16 length, the JavaScript `string.length`.
pub fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

pub fn estimate_string_tokens(text: &str) -> u64 {
    js_len(text).div_ceil(4) as u64
}

pub fn observation_line(id: &str, timestamp: &str, relevance: &str, content: &str) -> String {
    format!("[{id}] {timestamp} [{relevance}] {content}")
}

pub fn observation_line_token_count(o: &Observation) -> u64 {
    estimate_string_tokens(&observation_line(
        &o.id,
        &o.timestamp,
        &o.relevance,
        &o.content,
    ))
}

fn str_len(value: Option<&Value>) -> usize {
    value.and_then(Value::as_str).map_or(0, js_len)
}

fn blocks_chars(content: Option<&Value>) -> usize {
    match content {
        Some(Value::String(text)) => js_len(text),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .map(|block| match block.get("type").and_then(Value::as_str) {
                Some("text") => str_len(block.get("text")),
                Some("image") => ESTIMATED_IMAGE_CHARS,
                _ => 0,
            })
            .sum(),
        _ => 0,
    }
}

fn assistant_chars(content: Option<&Value>) -> usize {
    let Some(Value::Array(blocks)) = content else {
        return 0;
    };
    blocks
        .iter()
        .map(|block| match block.get("type").and_then(Value::as_str) {
            Some("text") => str_len(block.get("text")),
            Some("thinking") => str_len(block.get("thinking")),
            Some("toolCall") => {
                let args = block.get("arguments").cloned().unwrap_or(Value::Null);
                str_len(block.get("name")) + js_len(&args.to_string())
            }
            _ => 0,
        })
        .sum()
}

/// pi's `estimateTokens(message)`: characters / 4, rounded up.
pub fn estimate_message_tokens(message: &Value) -> u64 {
    let content = message.get("content");
    let chars = match message.get("role").and_then(Value::as_str) {
        Some("assistant") => assistant_chars(content),
        Some("user") | Some("toolResult") | Some("custom") => blocks_chars(content),
        Some("bashExecution") => str_len(message.get("command")) + str_len(message.get("output")),
        Some("branchSummary") | Some("compactionSummary") => str_len(message.get("summary")),
        _ => 0,
    };
    chars.div_ceil(4) as u64
}

pub fn estimate_entry_tokens(entry: &Value) -> u64 {
    match entry_type(entry) {
        "message" => entry.get("message").map_or(0, estimate_message_tokens),
        "custom_message" => match entry.get("content") {
            Some(Value::String(text)) => estimate_string_tokens(text),
            Some(Value::Array(blocks)) => blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .map(estimate_string_tokens)
                .sum(),
            _ => 0,
        },
        "branch_summary" => entry
            .get("summary")
            .and_then(Value::as_str)
            .map_or(0, estimate_string_tokens),
        _ => 0,
    }
}
