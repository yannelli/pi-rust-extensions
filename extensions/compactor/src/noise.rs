//! Noise filter: thinking, noise tools, and user blocks that are only wrappers or boilerplate.

use std::sync::LazyLock;

use regex::Regex;

use crate::text::{Block, Kind};

const NOISE_TOOLS: &[&str] = &[
    "TodoWrite",
    "TodoRead",
    "ToolSearch",
    "WebSearch",
    "AskUser",
    "ExitSpecMode",
    "GenerateDroid",
];

const NOISE_STRINGS: &[&str] = &[
    "Continue from where you left off.",
    "No response requested.",
    "IMPORTANT: TodoWrite was not called yet.",
];

static XML_WRAPPER_RE: LazyLock<Regex> = LazyLock::new(|| {
    let tags = [
        "system-reminder",
        "ide_opened_file",
        "command-message",
        "context-window-usage",
    ];
    let alternatives: Vec<String> = tags
        .iter()
        .map(|t| format!(r"<{t}[^>]*>[\s\S]*?</{t}>"))
        .collect();
    Regex::new(&alternatives.join("|")).expect("xml wrapper regex")
});

/// Drops thinking, noise tools, and user blocks that are only wrappers or boilerplate.
pub fn filter_noise(blocks: Vec<Block>) -> Vec<Block> {
    let mut out = Vec::with_capacity(blocks.len());
    for mut block in blocks {
        match block.kind {
            Kind::Thinking => continue,
            Kind::ToolCall | Kind::ToolResult if NOISE_TOOLS.contains(&block.name.as_str()) => {
                continue;
            }
            Kind::User => {
                let trimmed = block.text.trim();
                if NOISE_STRINGS.iter().any(|s| trimmed.contains(s)) {
                    continue;
                }
                let cleaned = XML_WRAPPER_RE.replace_all(trimmed, "").trim().to_owned();
                if cleaned.is_empty() {
                    continue;
                }
                block.text = cleaned;
            }
            _ => {}
        }
        out.push(block);
    }
    out
}
