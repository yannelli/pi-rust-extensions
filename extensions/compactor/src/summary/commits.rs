//! Commits: git summary lines in tool results, and quiet `git commit -m` subjects.

use std::sync::LazyLock;

use regex::Regex;

use crate::summary::files::fit_items;
use crate::text::{Block, Kind, clip};

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("commit regex")
}

static COMMIT_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| re(r"\[([\w./@-]+)(?: \(root-commit\))? ([0-9a-f]{7,40})\] "));
static FILES_CHANGED: LazyLock<Regex> = LazyLock::new(|| re(r" \d+ files? changed"));
static TRAILER: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\s+(?:Date|Author|Co-authored-by):.*$"));
static COMMIT_CMD: LazyLock<Regex> = LazyLock::new(|| {
    re(r#"\bgit\s+commit\b[^"'\n]*?\s-[a-zA-Z]*m\s*(?:"([^"\n]{3,}?)"|'([^'\n]{3,}?)')"#)
});

/// `(hash, subject)` of git commit summary lines (`[branch abc1234] subject 1 file changed`).
pub fn commit_lines(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(caps) = COMMIT_PREFIX.captures_at(text, at) {
        let whole = caps.get(0).expect("match");
        let line_end = text[whole.end()..]
            .find('\n')
            .map_or(text.len(), |i| whole.end() + i);
        let body = &text[whole.end()..line_end];
        let subject_end = body
            .char_indices()
            .nth(1)
            .and_then(|(first, _)| FILES_CHANGED.find_at(body, first))
            .map(|m| m.start())
            .or((line_end == text.len() && !body.is_empty()).then_some(body.len()));
        at = whole.end();
        if let Some(end) = subject_end {
            out.push((caps[2].to_owned(), body[..end].to_owned()));
            at = whole.end() + end;
        }
    }
    out
}

pub fn select_commits(blocks: &[Block], max_chars: usize) -> Vec<String> {
    let mut commits: Vec<(String, String)> = Vec::new();
    let set = |key: String, subject: String, commits: &mut Vec<(String, String)>| match commits
        .iter_mut()
        .find(|(k, _)| *k == key)
    {
        Some(entry) => entry.1 = subject,
        None => commits.push((key, subject)),
    };
    let mut pending: Option<String> = None;
    for block in blocks {
        if block.kind == Kind::ToolCall {
            let command = block.arg_str("command").unwrap_or_default();
            pending = COMMIT_CMD
                .captures(command)
                .and_then(|c| c.get(1).or(c.get(2)))
                .map(|m| m.as_str().to_owned());
            continue;
        }
        if block.kind != Kind::ToolResult {
            continue;
        }
        let mut found = false;
        if !block.is_error {
            for (hash, subject) in commit_lines(&block.text) {
                let subject = TRAILER.replace(&subject, "").trim().to_owned();
                set(
                    hash.chars().take(8).collect(),
                    clip(&subject, 110),
                    &mut commits,
                );
                found = true;
            }
        }
        if let Some(subject) = pending.take().filter(|_| !found && !block.is_error) {
            let key = format!("~{}", commits.len());
            set(key, clip(subject.trim(), 110), &mut commits);
        }
    }
    let lines: Vec<String> = commits
        .into_iter()
        .map(|(hash, subject)| {
            if hash.starts_with('~') {
                subject
            } else {
                format!("{hash} {subject}")
            }
        })
        .collect();
    let mut newest: Vec<String> = lines[lines.len().saturating_sub(10)..]
        .iter()
        .rev()
        .cloned()
        .collect();
    newest = fit_items(&newest, max_chars);
    newest.reverse();
    newest
}
