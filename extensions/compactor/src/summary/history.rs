//! Lessons, Commits, Open Errors, and Recent Transcript.

use std::sync::LazyLock;

use regex::Regex;

use crate::brief::compile_brief;
use crate::rank::{FileOps, select_ranked};
use crate::summary::files::fit_items;
use crate::text::{Block, Kind, char_len, clip, extract_path, one_line};

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("history regex")
}

pub const TRANSCRIPT_WINDOW_BLOCKS: usize = 160;
const RESOLVED_AFTER_SUCCESSES: usize = 3;

static MEMORY_TOOL: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^(?:memory_store|memory_save|memory_add|save_memory|remember)$"));
static LESSON_MARK: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\b(?:root cause|turns out|found it|confirmed|gotcha|caveat|silently|no-?ops?|doesn'?t (?:support|work|accept)|not supported|only works|rejects?|ignores?|the fix (?:is|was)|workaround|breaks? (?:when|if)|must (?:use|be|run)|requires?|instead of|never|always)\b",
    )
});
static DIAGNOSIS: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\b(?:root cause|turns out|gotcha|caveat|silently|no-?ops?|doesn'?t (?:support|accept)|not supported|only works|rejects?|ignores?|the (?:real )?(?:issue|problem|bug|cause) (?:is|was)|the fix (?:is|was)|workaround)\b",
    )
});
static DEBUG: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\b(?:DBG|debug(?:ging)? (?:print|log|output)|print(?:ed)? (?:never|didn'?t)|never (?:fired|printed)|let me|I'?ll (?:check|look|try))\b",
    )
});
static CONFIRMED: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\b(?:confirmed|found it)\b"));
static COMMENT: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*#+\s*(.+)$"));
static CLOSED_FENCE: LazyLock<Regex> = LazyLock::new(|| re(r"```[\s\S]*?```"));
static PUNCT_SPACE: LazyLock<Regex> = LazyLock::new(|| re(r"[.!?]\s+|\n+"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LessonSource {
    Memory,
    Comment,
    Diagnosis,
}

fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for m in PUNCT_SPACE.find_iter(text) {
        let keep = if m.as_str().starts_with('\n') {
            m.start()
        } else {
            m.start() + 1
        };
        out.push(&text[start..keep]);
        start = m.end();
    }
    out.push(&text[start..]);
    out
}

/// Agent-written lessons in session order; a later duplicate moves to the end.
pub fn lesson_candidates(blocks: &[Block]) -> Vec<(String, LessonSource)> {
    let mut out: Vec<(String, String, LessonSource)> = Vec::new();
    let mut add = |text: &str, source: LessonSource| {
        let t = one_line(text);
        if char_len(&t) < 30 {
            return;
        }
        let key: String = t.chars().take(80).collect::<String>().to_lowercase();
        out.retain(|(k, _, _)| *k != key);
        out.push((key, clip(&t, 320), source));
    };
    for block in blocks {
        if block.kind == Kind::ToolCall && MEMORY_TOOL.is_match(&block.name) {
            let raw = ["content", "text", "memory", "note"]
                .iter()
                .find_map(|k| block.arg_str(k));
            if let Some(raw) = raw {
                let mut text = String::new();
                for line in raw.split('\n').map(str::trim).filter(|l| !l.is_empty()) {
                    text = if text.is_empty() {
                        line.to_owned()
                    } else {
                        format!("{text} {line}")
                    };
                    if char_len(&text) >= 160 {
                        break;
                    }
                }
                add(&text, LessonSource::Memory);
            }
        } else if block.kind == Kind::ToolCall
            && let Some(command) = block.arg_str("command")
        {
            for line in command.split('\n') {
                let Some(caps) = COMMENT.captures(line) else {
                    continue;
                };
                let c = &caps[1];
                let marked = DIAGNOSIS.is_match(c) || CONFIRMED.is_match(c);
                if char_len(c) <= 300
                    && !c.trim_end().ends_with(':')
                    && !DEBUG.is_match(c)
                    && marked
                    && LESSON_MARK.is_match(c)
                {
                    add(c, LessonSource::Comment);
                }
            }
        } else if block.kind == Kind::Assistant {
            let text = CLOSED_FENCE.replace_all(&block.text, " ");
            for sentence in sentences(&text) {
                let t = sentence.trim();
                let len = char_len(t);
                if (50..=320).contains(&len)
                    && !t.ends_with('?')
                    && !DEBUG.is_match(t)
                    && DIAGNOSIS.is_match(t)
                {
                    add(t, LessonSource::Diagnosis);
                }
            }
        }
    }
    out.into_iter()
        .map(|(_, text, source)| (text, source))
        .collect()
}

/// Memory notes first, then comments, then diagnoses; newest first in each; session order out.
pub fn select_lessons(blocks: &[Block], max_chars: usize) -> Vec<String> {
    let all = lesson_candidates(blocks);
    let mut ranked: Vec<usize> = (0..all.len()).collect();
    ranked.sort_by(|a, b| all[*a].1.cmp(&all[*b].1).then(b.cmp(a)));
    let mut chosen = Vec::new();
    let mut used = 0;
    for i in ranked {
        let cost = char_len(&all[i].0) + 3;
        if used + cost > max_chars {
            continue;
        }
        chosen.push(i);
        used += cost;
    }
    chosen.sort_unstable();
    chosen.into_iter().map(|i| all[i].0.clone()).collect()
}

fn call_target(block: &Block) -> String {
    extract_path(&block.args)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            one_line(block.arg_str("command").unwrap_or_default())
                .chars()
                .take(60)
                .collect()
        })
}

struct OpenError {
    key: String,
    tool: String,
    text: String,
    source: Option<usize>,
    successes_after: usize,
}

pub fn select_open_errors(blocks: &[Block], max_chars: usize) -> Vec<String> {
    let mut last_target: Vec<(String, String)> = Vec::new();
    let mut open: Vec<OpenError> = Vec::new();
    for block in &blocks[blocks.len().saturating_sub(TRANSCRIPT_WINDOW_BLOCKS)..] {
        if block.kind == Kind::ToolCall {
            last_target.retain(|(name, _)| *name != block.name);
            last_target.push((block.name.clone(), call_target(block)));
            continue;
        }
        if block.kind != Kind::ToolResult {
            continue;
        }
        let target = last_target
            .iter()
            .find(|(n, _)| *n == block.name)
            .map(|(_, t)| t.as_str())
            .unwrap_or_default();
        let key = format!("{}\u{0}{target}", block.name);
        open.retain(|e| e.key != key);
        if block.is_error {
            let text = format!("[{}] {}", block.name, clip(&one_line(&block.text), 200));
            open.push(OpenError {
                key,
                tool: block.name.clone(),
                text,
                source: block.source,
                successes_after: 0,
            });
            continue;
        }
        for err in open.iter_mut().filter(|e| e.tool == block.name) {
            err.successes_after += 1;
        }
        open.retain(|e| e.tool != block.name || e.successes_after < RESOLVED_AFTER_SUCCESSES);
    }
    let lines: Vec<String> = open
        .iter()
        .map(|e| {
            e.source
                .map_or_else(|| e.text.clone(), |s| format!("{} (#{s})", e.text))
        })
        .collect();
    let newest: Vec<String> = lines[lines.len().saturating_sub(5)..]
        .iter()
        .rev()
        .cloned()
        .collect();
    let mut fitted = fit_items(&newest, max_chars);
    fitted.reverse();
    fitted
}

/// Recent ranked blocks as a brief transcript, newest lines kept under `max_chars`.
pub fn select_transcript(blocks: &[Block], max_chars: usize, ops: &FileOps) -> String {
    if max_chars < 200 || blocks.is_empty() {
        return String::new();
    }
    let recent = &blocks[blocks.len().saturating_sub(TRANSCRIPT_WINDOW_BLOCKS)..];
    let selected = select_ranked(recent, max_chars, ops);
    let lines: Vec<String> = compile_brief(&selected)
        .split('\n')
        .map(|l| clip(l, 300))
        .collect();
    let mut out: Vec<String> = Vec::new();
    let mut used = 0;
    for line in lines.iter().rev() {
        let cost = char_len(line) + 1;
        if used + cost > max_chars {
            break;
        }
        out.insert(0, line.clone());
        used += cost;
    }
    while out.first().is_some_and(|l| !l.starts_with('[')) {
        out.remove(0);
    }
    out.join("\n").trim().to_owned()
}
