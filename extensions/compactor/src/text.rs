//! Session entries to normalized blocks: text extraction, sanitizing, noise filter.

use serde_json::Value;

pub use crate::noise::filter_noise;

pub const ELLIPSIS: char = '\u{2026}';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    User,
    Assistant,
    ToolCall,
    ToolResult,
    Thinking,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::User => "user",
            Kind::Assistant => "assistant",
            Kind::ToolCall => "tool_call",
            Kind::ToolResult => "tool_result",
            Kind::Thinking => "thinking",
        }
    }
}

/// One normalized message part. `source` is the branch entry index.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub kind: Kind,
    pub text: String,
    pub name: String,
    pub args: Value,
    pub is_error: bool,
    pub source: Option<usize>,
}

impl Block {
    pub fn text(kind: Kind, text: impl Into<String>, source: usize) -> Self {
        Self {
            kind,
            text: text.into(),
            name: String::new(),
            args: Value::Null,
            is_error: false,
            source: Some(source),
        }
    }

    pub fn call(name: &str, args: Value, source: usize) -> Self {
        Self {
            name: name.to_owned(),
            args,
            ..Self::text(Kind::ToolCall, "", source)
        }
    }

    pub fn result(name: &str, text: String, is_error: bool, source: usize) -> Self {
        Self {
            name: name.to_owned(),
            is_error,
            ..Self::text(Kind::ToolResult, text, source)
        }
    }

    pub fn arg_str(&self, key: &str) -> Option<&str> {
        self.args.get(key).and_then(Value::as_str)
    }
}

pub fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// Plain text of message content, with markers for non-text parts.
pub fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|part| match part.get("type").and_then(Value::as_str) {
                Some("text") => str_of(part, "text").to_owned(),
                Some("toolCall") => format!("[toolCall:{}]", str_of(part, "name")),
                Some("thinking") => "[thinking]".to_owned(),
                Some("image") => format!("[image:{}]", str_of(part, "mimeType")),
                Some(other) => format!("[{other}]"),
                None => "[unknown]".to_owned(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Strips control characters and collapses all whitespace to single spaces.
pub fn sanitize(text: &str) -> String {
    let kept: String = text
        .chars()
        .filter(|c| !matches!(*c as u32, 0x00..=0x08 | 0x0b..=0x0c | 0x0e..=0x1f))
        .collect();
    one_line(&kept)
}

pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Keeps `max` chars; the last one is an ellipsis when the text was cut.
pub fn clip(text: &str, max: usize) -> String {
    if max <= 1 {
        return String::new();
    }
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max - 1).collect();
    format!("{}{ELLIPSIS}", head.trim_end())
}

pub fn char_len(text: &str) -> usize {
    text.chars().count()
}

/// First path-like argument of a tool call.
pub fn extract_path(args: &Value) -> Option<&str> {
    ["file_path", "path", "filePath", "filepath", "file"]
        .iter()
        .find_map(|key| args.get(*key).and_then(Value::as_str))
        .filter(|path| !path.is_empty())
}

fn normalize_llm(message: &Value, source: usize) -> Vec<Block> {
    let content = &message["content"];
    match str_of(message, "role") {
        "user" => {
            let mut blocks = Vec::new();
            let text = sanitize(&text_of(content));
            if !text.is_empty() {
                blocks.push(Block::text(Kind::User, text, source));
            }
            for part in content.as_array().into_iter().flatten() {
                if part["type"] == "image" {
                    let marker = format!("[image: {}]", str_of(part, "mimeType"));
                    blocks.push(Block::text(Kind::User, marker, source));
                }
            }
            if blocks.is_empty() {
                blocks.push(Block::text(Kind::User, "", source));
            }
            blocks
        }
        "toolResult" => {
            let is_error = message["isError"].as_bool().unwrap_or(false);
            let text = sanitize(&text_of(content));
            vec![Block::result(
                str_of(message, "toolName"),
                text,
                is_error,
                source,
            )]
        }
        "assistant" => match content {
            Value::String(text) => vec![Block::text(Kind::Assistant, sanitize(text), source)],
            Value::Array(parts) => parts
                .iter()
                .filter_map(|part| assistant_part(part, source))
                .collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn assistant_part(part: &Value, source: usize) -> Option<Block> {
    match str_of(part, "type") {
        "text" => Some(Block::text(
            Kind::Assistant,
            sanitize(str_of(part, "text")),
            source,
        )),
        "thinking" => {
            let mut block = Block::text(Kind::Thinking, sanitize(str_of(part, "thinking")), source);
            block.is_error = part["redacted"].as_bool().unwrap_or(false);
            Some(block)
        }
        "toolCall" => Some(Block::call(
            str_of(part, "name"),
            part["arguments"].clone(),
            source,
        )),
        _ => None,
    }
}

fn labeled(kind: Kind, parts: [&str; 2], source: usize) -> Vec<Block> {
    let joined: Vec<&str> = parts.into_iter().filter(|p| !p.is_empty()).collect();
    let clean = sanitize(&joined.join("\n"));
    if clean.is_empty() {
        Vec::new()
    } else {
        vec![Block::text(kind, clean, source)]
    }
}

/// Blocks of one agent message, pi-specific roles included.
pub fn normalize_agent_message(message: &Value, source: usize) -> Vec<Block> {
    match str_of(message, "role") {
        "user" | "assistant" | "toolResult" => normalize_llm(message, source),
        "bashExecution" => {
            let parts = [
                format!("$ {}", str_of(message, "command")),
                str_of(message, "output").to_owned(),
            ];
            let text: Vec<&str> = parts
                .iter()
                .map(String::as_str)
                .filter(|p| !p.is_empty())
                .collect();
            let cancelled = message["cancelled"].as_bool().unwrap_or(false);
            let failed = message["exitCode"].as_i64().unwrap_or(0) != 0;
            vec![Block::result(
                "bash",
                sanitize(&text.join("\n")),
                cancelled || failed,
                source,
            )]
        }
        "custom" => {
            let text = text_of(&message["content"]);
            labeled(Kind::User, [str_of(message, "customType"), &text], source)
        }
        "branchSummary" | "compactionSummary" => {
            labeled(Kind::Assistant, [str_of(message, "summary"), ""], source)
        }
        _ => Vec::new(),
    }
}

/// Searchable blocks of a session branch (raw messages kept after compactions).
pub fn recall_blocks(entries: &[Value]) -> Vec<Block> {
    let mut blocks = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        match str_of(entry, "type") {
            "message" => blocks.extend(normalize_agent_message(&entry["message"], i)),
            "custom_message" => {
                let text = text_of(&entry["content"]);
                blocks.extend(labeled(Kind::User, [str_of(entry, "customType"), &text], i));
            }
            "branch_summary" | "compaction" => {
                blocks.extend(labeled(Kind::Assistant, [str_of(entry, "summary"), ""], i));
            }
            _ => {}
        }
    }
    filter_noise(blocks)
}
