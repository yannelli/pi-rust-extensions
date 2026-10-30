//! Token estimates: per-session chars-per-token calibration and content sizes.

use serde_json::Value;

use crate::text::{char_len, str_of};

pub const DEFAULT_CHARS_PER_TOKEN: f64 = 4.0;
pub const IMAGE_CONTENT_CHARS: usize = 4800;

/// Chars per token from pi's real count, clamped to 2..6; 4 without data.
pub fn calibrate_chars_per_token(chars: usize, tokens: Option<u64>) -> f64 {
    match tokens {
        Some(tokens) if tokens > 0 && chars > 0 => (chars as f64 / tokens as f64).clamp(2.0, 6.0),
        _ => DEFAULT_CHARS_PER_TOKEN,
    }
}

pub fn tokens_from_chars(chars: usize, chars_per_token: f64) -> u64 {
    (chars as f64 / chars_per_token).ceil() as u64
}

/// Token-bearing chars of message content: text, thinking, tool calls, images.
pub fn content_chars(content: &Value) -> usize {
    let parts = match content {
        Value::String(text) => return char_len(text),
        Value::Array(parts) => parts,
        _ => return 0,
    };
    parts
        .iter()
        .map(|part| match str_of(part, "type") {
            "thinking" => char_len(str_of(part, "thinking")),
            "toolCall" => {
                let args = part
                    .get("arguments")
                    .or_else(|| part.get("input"))
                    .unwrap_or(&Value::Null);
                let len = args
                    .as_str()
                    .map_or_else(|| char_len(&args.to_string()), char_len);
                char_len(str_of(part, "name")) + len
            }
            "toolResult" => match &part["content"] {
                Value::String(text) => char_len(text),
                other => char_len(&other.to_string()),
            },
            "image" => IMAGE_CONTENT_CHARS,
            _ => char_len(str_of(part, "text")),
        })
        .sum()
}
