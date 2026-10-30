//! What the lossless summary is built from, rebuilt from the full branch every time.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::text::{Block, Kind, filter_noise, normalize_agent_message, one_line, str_of, text_of};

/// Custom entry appended when an extension (not the user) sends a user message.
pub const ORIGIN_ENTRY_TYPE: &str = "compactor-origin";

/// Stable key of a user message text: FNV-1a over the first 400 chars of its one-line form.
pub fn origin_key(text: &str) -> String {
    let normalized: String = one_line(text).chars().take(400).collect();
    let hash = normalized
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
    format!("{hash:016x}")
}

static INJECTED: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"^[─━=\-]{8,}\s*🔄 RALPH LOOP",
        r"🔄 RALPH LOOP:",
        r"^No-progress guard:",
        r"^Continue working toward the active thread goal",
        r"^Goal (?:recovery|status audit)\b",
        r"^The active goal reached a budget limit",
        r"^\[kanboard [\w.-]+\]",
        r"^<[a-zA-Z][\w-]*(?:\s[^>]*)?>",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("injected pattern"))
    .collect()
});

/// User-role text that an extension sent: marked by origin, or a known injected shape.
pub fn is_injected_user_text(text: &str, origins: &HashSet<String>) -> bool {
    let trimmed = text.trim();
    trimmed.is_empty()
        || origins.contains(&origin_key(trimmed))
        || INJECTED.iter().any(|re| re.is_match(trimmed))
}

static PROGRESS_TOOLS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:ralph_done|update_goal)$").unwrap());
static ASK_TOOLS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:ask_user|ask_user_question|askuserquestion|ask)$").unwrap()
});
static ASK_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^User (?:wrote|selected|answered|chose)[^:]*:\s*([\s\S]+)$").unwrap()
});

fn short_question(question: &str) -> String {
    let one = one_line(question);
    if one.chars().count() > 140 {
        let head: String = one.chars().take(139).collect();
        format!("{}{}", head.trim_end(), crate::text::ELLIPSIS)
    } else {
        one
    }
}

/// "Q → A" lines from an ask tool result, one per answered question.
pub fn ask_answers(message: &Value) -> Vec<String> {
    let details = &message["details"];
    if let (Some(questions), Some(answers)) = (
        details["questions"].as_array(),
        details["answers"].as_array(),
    ) {
        let mut out = Vec::new();
        for (i, question) in questions.iter().enumerate() {
            let Some(answer) = answers
                .get(i)
                .filter(|a| a.is_object() && a["skipped"] != true)
            else {
                continue;
            };
            let options = question["options"].as_array();
            let mut parts: Vec<String> = answer["selected"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|value| {
                    let value_str = value.as_str().unwrap_or_default();
                    options
                        .and_then(|opts| {
                            opts.iter().find(|o| {
                                o.get("value")
                                    .or_else(|| o.get("label"))
                                    .and_then(Value::as_str)
                                    == Some(value_str)
                            })
                        })
                        .and_then(|o| o["label"].as_str())
                        .unwrap_or(value_str)
                        .to_owned()
                })
                .collect();
            if let Some(custom) = answer["custom_text"]
                .as_str()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                parts.push(custom.to_owned());
            }
            let joined = parts.join(", ");
            if !joined.is_empty() {
                out.push(format!(
                    "{} → {joined}",
                    short_question(str_of(question, "question"))
                ));
            }
        }
        return out;
    }
    let text = text_of(&message["content"]);
    let Some(caps) = ASK_TEXT.captures(text.trim()) else {
        return Vec::new();
    };
    let answer = caps[1].trim().to_owned();
    match details["question"].as_str() {
        Some(question) => vec![format!("{} → {answer}", short_question(question))],
        None => vec![answer],
    }
}

#[derive(Debug, Clone, Default)]
pub struct SummarySource {
    pub blocks: Vec<Block>,
    /// The user's own messages, raw text, oldest first.
    pub requests: Vec<String>,
    /// Assistant prose per message, oldest first.
    pub reports: Vec<String>,
    pub tail: Vec<Block>,
}

pub fn collect_origins(branch: &[Value]) -> HashSet<String> {
    branch
        .iter()
        .filter(|e| str_of(e, "type") == "custom" && str_of(e, "customType") == ORIGIN_ENTRY_TYPE)
        .filter_map(|e| e["data"]["key"].as_str().map(str::to_owned))
        .collect()
}

fn assistant_prose(message: &Value) -> String {
    match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|p| p["type"] == "text")
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_owned(),
        _ => String::new(),
    }
}

/// The summary source from branch entries `[0, end)`; compaction entries are skipped.
pub fn collect_summary_source(branch: &[Value], end: usize) -> SummarySource {
    let origins = collect_origins(branch);
    let end = end.min(branch.len());
    let mut source = SummarySource::default();
    let mut blocks = Vec::new();
    for (i, entry) in branch[..end].iter().enumerate() {
        if str_of(entry, "type") == "branch_summary" {
            if let Some(summary) = entry["summary"].as_str() {
                let text: String = one_line(summary).chars().take(600).collect();
                blocks.push(Block::text(
                    Kind::Assistant,
                    format!("[branch summary] {text}"),
                    i,
                ));
            }
            continue;
        }
        if str_of(entry, "type") != "message" || !entry["message"].is_object() {
            continue;
        }
        let message = &entry["message"];
        let tool = str_of(message, "toolName");
        match str_of(message, "role") {
            "user" => {
                let raw = text_of(&message["content"]).trim().to_owned();
                if is_injected_user_text(&raw, &origins) {
                    continue;
                }
                source.requests.push(raw);
            }
            "assistant" => {
                let prose = assistant_prose(message);
                if !prose.is_empty() {
                    source.reports.push(prose);
                }
            }
            "toolResult" if ASK_TOOLS.is_match(tool) => {
                source.requests.extend(ask_answers(message))
            }
            "toolResult" if PROGRESS_TOOLS.is_match(tool) => {
                let text = text_of(&message["content"]).trim().to_owned();
                if !text.is_empty() {
                    source.reports.push(text);
                }
            }
            "custom" => continue,
            _ => {}
        }
        blocks.extend(normalize_agent_message(message, i));
    }
    let tail: Vec<Block> = branch
        .iter()
        .enumerate()
        .skip(end)
        .filter(|(_, e)| {
            str_of(e, "type") == "message"
                && e["message"].is_object()
                && e["message"]["role"] != "custom"
        })
        .flat_map(|(i, e)| normalize_agent_message(&e["message"], i))
        .collect();
    source.blocks = filter_noise(blocks);
    source.tail = filter_noise(tail);
    source
}

/// Parsed `customInstructions` of `session_before_compact`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionArgs {
    pub is_compactor: bool,
    pub keep: usize,
    pub keep_explicit: bool,
    pub follow_up: Option<String>,
}

/// The marker that compactor commands put in `customInstructions`.
pub const COMPACTOR_INSTRUCTION: &str = "__compactor__";

static KEEP_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^keep:(\d+)(?:\s+|$)([\s\S]*)$").unwrap());
static KEEP_TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^keep:(\d+)$").unwrap());

/// `keep:N` at the start or end, and the remaining prompt text.
pub fn parse_keep_and_prompt(args: &str) -> (String, Option<usize>) {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return (String::new(), None);
    }
    let keep_of = |raw: &str| raw.parse::<usize>().unwrap_or(usize::MAX);
    if let Some(caps) = KEEP_START.captures(trimmed) {
        return (caps[2].trim().to_owned(), Some(keep_of(&caps[1])));
    }
    let last = trimmed.split_whitespace().last().unwrap_or_default();
    if let Some(caps) = KEEP_TOKEN.captures(last) {
        let prompt = trimmed[..trimmed.len() - last.len()].trim().to_owned();
        return (prompt, Some(keep_of(&caps[1])));
    }
    (trimmed.to_owned(), None)
}

pub fn parse_compaction_instructions(custom: Option<&str>) -> CompactionArgs {
    let trimmed = custom.map(str::trim).unwrap_or_default();
    if trimmed == COMPACTOR_INSTRUCTION {
        return CompactionArgs {
            is_compactor: true,
            keep: 1,
            keep_explicit: false,
            follow_up: None,
        };
    }
    if let Some(rest) = trimmed
        .strip_prefix(COMPACTOR_INSTRUCTION)
        .filter(|r| r.starts_with(char::is_whitespace))
    {
        let first_line = rest.trim().split('\n').next().unwrap_or_default();
        let (_, keep) = parse_keep_and_prompt(first_line);
        return CompactionArgs {
            is_compactor: true,
            keep: keep.unwrap_or(1),
            keep_explicit: keep.is_some(),
            follow_up: None,
        };
    }
    let (prompt, keep) = parse_keep_and_prompt(custom.unwrap_or_default());
    CompactionArgs {
        is_compactor: false,
        keep: keep.unwrap_or(1),
        keep_explicit: keep.is_some(),
        follow_up: Some(prompt).filter(|p| !p.is_empty()),
    }
}
