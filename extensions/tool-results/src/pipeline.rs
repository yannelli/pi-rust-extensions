//! Runs the enabled transforms over the text blocks of one tool result.

use pi_ext_sdk::content;
use serde_json::Value;

use crate::config::Settings;
use crate::lines::{group, short};
use crate::{blobs, clean, jsonfmt, lines, stack};

pub const GET_TOOL: &str = "tool_result_get";

#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub content: Vec<Value>,
    pub before: usize,
    pub after: usize,
    pub applied: Vec<String>,
    /// The full text after cleanup, to store under the result id.
    pub stored: Option<String>,
    pub truncated: bool,
}

struct Block {
    cleaned: String,
    text: String,
    lossy: bool,
}

fn step(
    text: &mut String,
    applied: &mut Vec<String>,
    name: &str,
    transform: impl FnOnce(&str) -> Option<String>,
) -> bool {
    match transform(text) {
        Some(out) => {
            *text = out;
            if !applied.iter().any(|a| a == name) {
                applied.push(name.to_owned());
            }
            true
        }
        None => false,
    }
}

fn block(raw: &str, s: &Settings, applied: &mut Vec<String>) -> Block {
    let mut text = raw.to_owned();
    if s.ansi {
        step(&mut text, applied, "ansi", clean::strip_ansi);
    }
    if s.progress {
        step(&mut text, applied, "progress", clean::progress);
    }
    if s.control {
        let control = |t: &str| clean::control(t, !s.progress, !s.ansi);
        step(&mut text, applied, "control", control);
    }
    let cleaned = text.clone();
    let json_name = format!("json:{}", s.json.as_str());
    if step(&mut text, applied, &json_name, |t| jsonfmt::convert(t, s)) {
        return Block {
            cleaned,
            text,
            lossy: false,
        };
    }
    if jsonfmt::parse(&cleaned).is_some() {
        return Block {
            cleaned,
            text,
            lossy: false,
        };
    }
    let mut lossy = false;
    if s.base64 {
        lossy |= step(&mut text, applied, "base64", |t| {
            blobs::elide(t, s.base64_min)
        });
    }
    if s.stack_traces {
        if let Some((out, folded)) = stack::fold(&text, s.stack_max_frames) {
            lossy |= folded;
            step(&mut text, applied, "stackTraces", |_| Some(out));
        }
    }
    if s.dedupe {
        step(&mut text, applied, "dedupe", |t| {
            lines::dedupe(t, s.dedupe_min)
        });
    }
    if s.long_lines {
        let clip = |t: &str| lines::long_lines(t, s.long_line_chars);
        lossy |= step(&mut text, applied, "longLines", clip);
    }
    if s.collapse_spaces {
        step(&mut text, applied, "collapseSpaces", lines::collapse_spaces);
    }
    if s.whitespace {
        step(&mut text, applied, "whitespace", lines::whitespace);
    }
    Block {
        cleaned,
        text,
        lossy,
    }
}

/// Up to `n` characters from the start, cut at a line end in the last quarter when there is one.
fn head(text: &str, n: usize) -> &str {
    let end = text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
    let cut = &text[..end];
    match cut.rfind('\n') {
        _ if end == text.len() => text,
        Some(at) if at * 4 >= end * 3 => &text[..at],
        _ => cut,
    }
}

fn tail(text: &str, n: usize) -> &str {
    let count = text.chars().count();
    let start = text
        .char_indices()
        .nth(count.saturating_sub(n))
        .map_or(text.len(), |(i, _)| i);
    let cut = &text[start..];
    match cut.find('\n') {
        Some(at) if at * 4 <= cut.len() => &cut[at + 1..],
        _ => cut,
    }
}

pub fn notice(total: usize, lines: usize, id: &str, context: usize) -> String {
    format!(
        "Result truncated ({} characters, {} lines). Use `{GET_TOOL}` with result_id \"{id}\" \
         to read a line range, search for a pattern and return matches with about {context} \
         characters of context, or return it in full.",
        short(total),
        group(lines)
    )
}

fn preview(text: &str, s: &Settings, stored: &str, id: &str, context: usize) -> String {
    let total = text.chars().count();
    let head = head(text, s.head_chars);
    let tail = tail(
        text,
        s.tail_chars.min(total.saturating_sub(head.chars().count())),
    );
    let omitted = total - head.chars().count() - tail.chars().count();
    let mut out = String::from(head);
    out.push_str(&format!(
        "\n\n[... {} characters omitted ...]\n\n",
        group(omitted)
    ));
    out.push_str(tail);
    let notice = notice(stored.chars().count(), stored.lines().count(), id, context);
    out.push_str(&format!("\n\n{notice}"));
    out
}

/// The new content, or `None` when nothing changed or nothing got smaller.
pub fn process(blocks: &[Value], s: &Settings, id: &str, context: usize) -> Option<Outcome> {
    let mut applied = Vec::new();
    let mut texts = Vec::new();
    let mut before = 0;
    for item in blocks {
        if item["type"] == "text" {
            let raw = item["text"].as_str().unwrap_or_default();
            before += raw.chars().count();
            texts.push(block(raw, s, &mut applied));
        }
    }
    if texts.is_empty() {
        return None;
    }
    let joined = texts
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let cleaned = || {
        texts
            .iter()
            .map(|b| b.cleaned.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let size = joined.chars().count();
    let mut outcome = Outcome {
        before,
        ..Outcome::default()
    };
    let others = blocks.iter().filter(|b| b["type"] != "text").cloned();
    if s.truncate && size > s.threshold {
        let stored = cleaned();
        let text = preview(&joined, s, &stored, id, context);
        outcome.content = std::iter::once(content::text(text)).chain(others).collect();
        outcome.stored = Some(stored);
        outcome.truncated = true;
        applied.push("truncate".into());
    } else {
        if applied.is_empty() {
            return None;
        }
        let mut texts_iter = texts.iter();
        outcome.content = blocks
            .iter()
            .map(|b| match b["type"] == "text" {
                true => content::text(texts_iter.next().map_or("", |t| t.text.as_str())),
                false => b.clone(),
            })
            .collect();
        if s.truncate && s.keep_elided && texts.iter().any(|b| b.lossy) {
            let note = format!(
                "\n\n[Some parts were elided. Use `{GET_TOOL}` with result_id \"{id}\" to read the full result.]"
            );
            if let Some(last) = outcome
                .content
                .iter_mut()
                .rev()
                .find(|b| b["type"] == "text")
            {
                let text = format!("{}{note}", last["text"].as_str().unwrap_or_default());
                *last = content::text(text);
            }
            outcome.stored = Some(cleaned());
        }
    }
    outcome.after = outcome
        .content
        .iter()
        .filter(|b| b["type"] == "text")
        .map(|b| b["text"].as_str().unwrap_or_default().chars().count())
        .sum();
    if !outcome.truncated && outcome.after >= before {
        return None;
    }
    outcome.applied = applied;
    Some(outcome)
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;
