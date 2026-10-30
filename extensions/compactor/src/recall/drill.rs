//! `#N:path` drill-down into file content carried by tool calls of entry N.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::text::{Block, Kind};

const PATH_KEYS: [&str; 4] = ["path", "file_path", "filePath", "file"];
const PREVIEW_LINES: usize = 30;
const MAX_FULL_BYTES: usize = 50 * 1024;

static DRILL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^#(\d+):(.+?)(?::(full|\d+(?::\d+)?))?$").unwrap());

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drill {
    pub index: usize,
    pub path: String,
    pub full: bool,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

pub fn parse_drill(query: &str) -> Option<Drill> {
    let caps = DRILL.captures(query)?;
    let mut drill = Drill {
        index: caps[1].parse().ok()?,
        path: caps[2].to_owned(),
        full: false,
        offset: None,
        limit: None,
    };
    match caps.get(3).map(|m| m.as_str()) {
        Some("full") => drill.full = true,
        Some(suffix) => {
            let mut parts = suffix.split(':');
            drill.offset = parts.next().and_then(|p| p.parse().ok());
            drill.limit = parts.next().and_then(|p| p.parse().ok());
        }
        None => {}
    }
    Some(drill)
}

fn path_of(args: &Value) -> Option<&str> {
    PATH_KEYS
        .iter()
        .find_map(|k| args.get(*k).and_then(Value::as_str))
        .filter(|p| !p.is_empty())
}

fn nonempty_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// The file body a tool call carries: `content`, `edits`, or `oldText`/`newText`.
fn body_of(args: &Value) -> Option<String> {
    path_of(args)?;
    if let Some(content) = nonempty_str(args, "content") {
        return Some(content.to_owned());
    }
    if let Some(edits) = args
        .get("edits")
        .and_then(Value::as_array)
        .filter(|e| !e.is_empty() && e.iter().all(Value::is_object))
    {
        let parts: Vec<String> = edits
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let old = e["oldText"].as_str().unwrap_or_default();
                let new = e["newText"].as_str().unwrap_or_default();
                format!("--- edit {} ---\n{old}\n--- becomes ---\n{new}", i + 1)
            })
            .collect();
        return Some(parts.join("\n\n"));
    }
    if args.get("edits").is_some() {
        return None;
    }
    match (nonempty_str(args, "oldText"), nonempty_str(args, "newText")) {
        (Some(old), Some(new)) => Some(format!("--- old ---\n{old}\n--- new ---\n{new}")),
        (Some(_), None) | (None, Some(_)) => {
            Some("(no file content found in tool call arguments)".into())
        }
        (None, None) => None,
    }
}

fn format_call(name: &str, path: &str, body: &str, n: usize, drill: &Drill) -> String {
    let header = format!("File: {path}\nTool: {name}\n\n");
    let lines: Vec<&str> = body.split('\n').collect();
    let total = lines.len();
    if drill.full {
        if body.len() > MAX_FULL_BYTES {
            let mut cut = MAX_FULL_BYTES;
            while !body.is_char_boundary(cut) {
                cut -= 1;
            }
            let more = body.len() - MAX_FULL_BYTES;
            return format!(
                "{header}{}\n\n... ({more} more bytes — file exceeds 50KB display limit. Use #{n}:{path}:{PREVIEW_LINES} for next page.)",
                &body[..cut]
            );
        }
        return format!("{header}{body}");
    }
    if let Some(offset) = drill.offset {
        let max = drill.limit.unwrap_or(30);
        let end = (offset + max).min(total);
        if offset >= end {
            return format!(
                "Offset {offset} is beyond file length {total}. Use #{n}:{path} for the first {PREVIEW_LINES} lines."
            );
        }
        let mut out = format!(
            "File: {path}\nTool: {name}\nLines {}-{end} (of {total}):\n\n{}",
            offset + 1,
            lines[offset..end].join("\n")
        );
        if end < total {
            out.push_str(&format!(
                "\n\n--- Use #{n}:{path}:{end} or #{n}:{path}:{end}:{max} for next {max} lines, #{n}:{path}:full for complete ---"
            ));
        } else if offset > 0 {
            out.push_str("\n\n(End of file)");
        }
        return out;
    }
    if total > PREVIEW_LINES {
        return format!(
            "{header}{}\n\n...({} more lines — use #{n}:{path}:full for complete content, or #{n}:{path}:{PREVIEW_LINES} for next {PREVIEW_LINES} lines)",
            lines[..PREVIEW_LINES].join("\n"),
            total - PREVIEW_LINES
        );
    }
    format!("{header}{body}")
}

pub fn expand_entry_file(blocks: &[Block], drill: &Drill) -> String {
    let n = drill.index;
    let entry: Vec<&Block> = blocks.iter().filter(|b| b.source == Some(n)).collect();
    if entry.is_empty() {
        return format!("Entry #{n} not found in session history.");
    }
    let calls: Vec<(&str, &str, String)> = entry
        .iter()
        .filter(|b| b.kind == Kind::ToolCall)
        .filter_map(|b| Some((b.name.as_str(), path_of(&b.args)?, body_of(&b.args)?)))
        .collect();
    let listing = |calls: &[&(&str, &str, String)]| {
        calls
            .iter()
            .map(|(name, path, _)| format!("  [#{n}:{path}] {name}({path})"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    if drill.path == "file" {
        return match calls.as_slice() {
            [] => format!("No file content found in entry #{n}."),
            [(name, path, body)] => format_call(name, path, body, n, drill),
            _ => format!(
                "Entry #{n} has {} file operations:\n{}\n\nUse #{n}:path to drill into a specific file.",
                calls.len(),
                listing(&calls.iter().collect::<Vec<_>>())
            ),
        };
    }
    let matched: Vec<&(&str, &str, String)> = calls
        .iter()
        .filter(|(_, path, _)| path.contains(&drill.path))
        .collect();
    match matched.as_slice() {
        [] => format!(
            "No file content found in entry #{n} for \"{}\".",
            drill.path
        ),
        [(name, path, body)] => format_call(name, path, body, n, drill),
        _ => format!(
            "Entry #{n} has {} file operations matching \"{}\":\n{}\n\nUse #{n}:<more-specific-path> to drill into a specific file.",
            matched.len(),
            drill.path,
            listing(&matched)
        ),
    }
}
