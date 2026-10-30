use chrono::{DateTime, Local, TimeZone};
use serde_json::Value;

use crate::ledger::{entry_id, entry_type};
use crate::tokens::{estimate_string_tokens, js_len};

const UNKNOWN_TIME: &str = "????-??-?? ??:??";
pub const MAX_RECORD_CONTENT_CHARS: usize = 10_000;
const SOURCE_OMISSION_MARKER: &str = "\n\n[… middle omitted: source exceeds observer input budget; original source remains in the session ledger …]\n\n";

fn fmt_local(d: DateTime<Local>) -> String {
    d.format("%Y-%m-%d %H:%M").to_string()
}

fn parse_time(value: &Value) -> Option<DateTime<Local>> {
    match value {
        Value::Number(n) => Local.timestamp_millis_opt(n.as_f64()? as i64).single(),
        Value::String(s) => DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Local)),
        _ => None,
    }
}

fn format_timestamp(value: Option<&Value>) -> String {
    value
        .and_then(parse_time)
        .map_or_else(|| UNKNOWN_TIME.to_owned(), fmt_local)
}

pub fn format_recall_timestamp(values: &[Option<&Value>]) -> String {
    values
        .iter()
        .flatten()
        .find_map(|v| parse_time(v))
        .map_or_else(|| "Unknown time".to_owned(), fmt_local)
}

pub fn now_timestamp() -> String {
    fmt_local(Local::now())
}

pub fn truncate_record_content(content: &str) -> String {
    let chars = content.chars().count();
    if chars <= MAX_RECORD_CONTENT_CHARS {
        return content.to_owned();
    }
    let head: String = content.chars().take(MAX_RECORD_CONTENT_CHARS).collect();
    let dropped = chars - MAX_RECORD_CONTENT_CHARS;
    format!("{head} … [truncated {dropped} chars]")
}

fn text_and_placeholders(content: Option<&Value>, thinking: bool) -> String {
    let blocks = match content {
        Some(Value::String(s)) => return s.clone(),
        Some(Value::Array(blocks)) => blocks,
        _ => return "[non-text content omitted]".to_owned(),
    };
    let omitted = || Some("[non-text content omitted]".to_owned());
    let parts: Vec<String> = blocks
        .iter()
        .filter_map(|block| {
            let str_of = |k: &str| block.get(k).and_then(Value::as_str);
            match str_of("type") {
                Some("text") if str_of("text").is_some() => str_of("text").map(str::to_owned),
                Some("thinking") => {
                    if thinking && block.get("redacted") == Some(&Value::Bool(true)) {
                        return None;
                    }
                    match str_of("thinking").filter(|_| thinking) {
                        Some(t) => Some(format!("[thinking: {t}]")),
                        None => omitted(),
                    }
                }
                Some("toolCall") if str_of("name").is_some() => {
                    let args = block
                        .get("arguments")
                        .cloned()
                        .unwrap_or(Value::Object(Default::default()));
                    Some(format!("[{}({args})]", str_of("name").unwrap_or_default()))
                }
                _ => omitted(),
            }
        })
        .collect();
    parts.join("\n")
}

fn text_only(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn non_empty_lines(text: &str) -> String {
    text.split('\n')
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn serialize_message(msg: &Value) -> Option<String> {
    let time = format_timestamp(msg.get("timestamp"));
    let content = msg.get("content");
    match msg.get("role").and_then(Value::as_str) {
        Some("user") => Some(format!("[User @ {time}]: {}", text_only(content))),
        Some("assistant") => {
            let body = non_empty_lines(&text_and_placeholders(content, true));
            (!body.is_empty()).then(|| format!("[Assistant @ {time}]: {body}"))
        }
        _ => {
            let tool = msg
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or("undefined");
            Some(format!(
                "[Tool result for {tool} @ {time}]: {}",
                text_only(content)
            ))
        }
    }
}

fn custom_message(entry: &Value, recall: bool) -> String {
    let custom = entry
        .get("customType")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let content = entry.get("content");
    if recall {
        let time = format_recall_timestamp(&[entry.get("timestamp")]);
        let origin = custom.map_or("Custom message".to_owned(), |c| {
            format!("Custom message ({c})")
        });
        return format!(
            "[{origin} @ {time}]: {}",
            text_and_placeholders(content, false)
        );
    }
    let time = format_timestamp(entry.get("timestamp"));
    let tag = custom.map_or("Custom".to_owned(), |c| format!("Custom ({c})"));
    format!("[{tag} @ {time}]: {}", text_only(content))
}

pub fn serialize_branch_entry(entry: &Value) -> Option<String> {
    match entry_type(entry) {
        "message" => entry.get("message").and_then(serialize_message),
        "custom_message" => Some(custom_message(entry, false)),
        "branch_summary" => {
            let summary = entry.get("summary")?.as_str()?;
            let time = format_timestamp(entry.get("timestamp"));
            Some(format!("[Branch summary @ {time}]: {summary}"))
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceChunk {
    pub text: String,
    pub source_entry_ids: Vec<String>,
    pub estimated_tokens: u64,
    pub truncated_source_entry_ids: Vec<String>,
}

fn slice_chars(s: &str, from: usize, to: usize) -> String {
    s.chars().skip(from).take(to.saturating_sub(from)).collect()
}

fn truncate_block(label: &str, rendered: &str, max_tokens: u64) -> Option<String> {
    let fixed = format!("{label}\n{SOURCE_OMISSION_MARKER}");
    if estimate_string_tokens(&fixed) > max_tokens {
        return None;
    }
    let full = format!("{label}\n{rendered}");
    if estimate_string_tokens(&full) <= max_tokens {
        return Some(full);
    }
    let retained = (max_tokens as usize * 4)
        .max(1)
        .saturating_sub(js_len(&fixed));
    let head = retained.div_ceil(2);
    let tail = retained - head;
    let len = rendered.chars().count();
    let tail_text = if tail > 0 {
        slice_chars(rendered, len.saturating_sub(tail), len)
    } else {
        String::new()
    };
    Some(format!(
        "{label}\n{}{SOURCE_OMISSION_MARKER}{tail_text}",
        slice_chars(rendered, 0, head)
    ))
}

/// Whole source entries up to the budget; a first entry over budget becomes a head/tail excerpt.
pub fn serialize_source_chunk(entries: &[Value], max_tokens: Option<u64>) -> SourceChunk {
    let mut blocks: Vec<String> = Vec::new();
    let mut chunk = SourceChunk::default();
    let mut estimated = 0;
    for entry in entries {
        let id = entry_id(entry);
        if id.is_empty() {
            continue;
        }
        let Some(rendered) = serialize_branch_entry(entry).filter(|r| !r.trim().is_empty()) else {
            continue;
        };
        let label = format!("[Source entry id: {id}]");
        let block = format!("{label}\n{rendered}");
        let separator = if blocks.is_empty() { "" } else { "\n\n" };
        let block_tokens = estimate_string_tokens(&format!("{separator}{block}"));
        if let Some(max) = max_tokens.filter(|max| estimated + block_tokens > *max) {
            if blocks.is_empty() {
                if let Some(excerpt) = truncate_block(&label, &rendered, max) {
                    blocks.push(excerpt);
                    chunk.source_entry_ids.push(id.to_owned());
                    chunk.truncated_source_entry_ids.push(id.to_owned());
                }
            }
            break;
        }
        blocks.push(block);
        chunk.source_entry_ids.push(id.to_owned());
        estimated += block_tokens;
    }
    chunk.text = blocks.join("\n\n");
    chunk.estimated_tokens = estimate_string_tokens(&chunk.text);
    chunk
}

fn recall_message(entry: &Value) -> Option<String> {
    let msg = entry.get("message").filter(|m| m.is_object())?;
    let time = format_recall_timestamp(&[msg.get("timestamp"), entry.get("timestamp")]);
    let content = msg.get("content");
    match msg.get("role").and_then(Value::as_str) {
        Some("user") => Some(format!(
            "[User @ {time}]: {}",
            text_and_placeholders(content, false)
        )),
        Some("assistant") => {
            let body = non_empty_lines(&text_and_placeholders(content, true));
            (!body.is_empty()).then(|| format!("[Assistant @ {time}]: {body}"))
        }
        _ => {
            let tool = msg
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or("undefined");
            Some(format!(
                "[Tool result: {tool} @ {time}]: {}",
                text_and_placeholders(content, false)
            ))
        }
    }
}

pub fn render_recall_source_entry(entry: &Value) -> Option<String> {
    match entry_type(entry) {
        "message" => recall_message(entry),
        "custom_message" => Some(custom_message(entry, true)),
        "branch_summary" => {
            let summary = entry.get("summary")?.as_str()?;
            let time = format_recall_timestamp(&[entry.get("timestamp")]);
            Some(format!("[Branch summary @ {time}]: {summary}"))
        }
        _ => None,
    }
}

pub fn render_recall_source_entries(entries: &[Value]) -> String {
    entries
        .iter()
        .filter_map(render_recall_source_entry)
        .filter(|b| !b.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}
