//! The `tool_result_get` tool: line ranges, pattern search with context, and full text.

use pi_ext_sdk::ToolSpec;
use regex::RegexBuilder;
use serde::Deserialize;
use serde_json::json;

use crate::lines::group;
use crate::pipeline::GET_TOOL;

const DEFAULT_LINES: usize = 200;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Args {
    pub result_id: String,
    pub mode: Option<String>,
    pub start: Option<usize>,
    pub end: Option<usize>,
    pub pattern: Option<String>,
    pub regex: bool,
    pub ignore_case: bool,
    pub context_chars: Option<usize>,
    pub max_matches: Option<usize>,
}

pub fn spec() -> ToolSpec {
    ToolSpec::new(
        GET_TOOL,
        "Read a large tool result that was stored instead of shown in full. mode \"lines\" \
         returns lines start..end (1-based, inclusive), \"search\" returns the matches of a \
         pattern with context around each, and \"full\" returns the whole result.",
    )
    .parameters(json!({
        "type": "object",
        "properties": {
            "result_id": { "type": "string", "description": "The result_id from the truncation notice" },
            "mode": { "type": "string", "enum": ["lines", "search", "full"], "description": "Default: lines" },
            "start": { "type": "integer", "minimum": 1, "description": "lines: first line (default 1)" },
            "end": { "type": "integer", "minimum": 1, "description": "lines: last line (default start + 199)" },
            "pattern": { "type": "string", "description": "search: the text to find" },
            "regex": { "type": "boolean", "description": "search: pattern is a regular expression" },
            "ignore_case": { "type": "boolean", "description": "search: ignore case" },
            "context_chars": { "type": "integer", "minimum": 0, "description": "search: characters of context on each side" },
            "max_matches": { "type": "integer", "minimum": 1, "description": "search: most matches to return" }
        },
        "required": ["result_id"]
    }))
}

pub fn lines(text: &str, id: &str, start: Option<usize>, end: Option<usize>) -> String {
    let all: Vec<&str> = text.lines().collect();
    let start = start.unwrap_or(1).max(1);
    let end = end
        .unwrap_or(start + DEFAULT_LINES - 1)
        .max(start)
        .min(all.len());
    if start > all.len() {
        return format!(
            "Result {id} has {} lines; line {start} is past the end.",
            group(all.len())
        );
    }
    let mut out = format!("Result {id}: lines {start}-{end} of {}\n", group(all.len()));
    for (n, line) in all[start - 1..end].iter().enumerate() {
        out.push_str(&format!("{}\t{line}\n", start + n));
    }
    out.truncate(out.trim_end_matches('\n').len());
    out
}

fn floor(text: &str, mut at: usize) -> usize {
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn ceil(text: &str, mut at: usize) -> usize {
    while at < text.len() && !text.is_char_boundary(at) {
        at += 1;
    }
    at.min(text.len())
}

/// Byte offset `context` characters before and after `from..to`.
fn window(text: &str, from: usize, to: usize, context: usize) -> (usize, usize) {
    let start = text[..from]
        .char_indices()
        .rev()
        .nth(context.saturating_sub(1))
        .map_or(0, |(i, _)| i);
    let start = if context == 0 { from } else { start };
    let end = text[to..]
        .char_indices()
        .nth(context)
        .map_or(text.len(), |(i, _)| to + i);
    (floor(text, start), ceil(text, end))
}

pub struct Search<'a> {
    pub pattern: &'a str,
    pub regex: bool,
    pub ignore_case: bool,
    pub context: usize,
    pub max: usize,
}

pub fn search(text: &str, id: &str, q: &Search) -> Result<String, String> {
    let source = if q.regex {
        q.pattern.to_owned()
    } else {
        regex::escape(q.pattern)
    };
    let re = RegexBuilder::new(&source)
        .case_insensitive(q.ignore_case)
        .size_limit(1 << 22)
        .build()
        .map_err(|err| format!("invalid pattern: {err}"))?;
    let found: Vec<(usize, usize)> = re
        .find_iter(text)
        .filter(|m| !m.is_empty())
        .map(|m| (m.start(), m.end()))
        .collect();
    if found.is_empty() {
        return Ok(format!("No matches for {:?} in result {id}.", q.pattern));
    }
    let mut windows: Vec<(usize, usize, usize, usize)> = Vec::new();
    for &(from, to) in found.iter().take(q.max.max(1)) {
        let (start, end) = window(text, from, to, q.context);
        match windows.last_mut() {
            Some(last) if start <= last.1 => {
                last.1 = last.1.max(end);
                last.3 += 1;
            }
            _ => windows.push((start, end, from, 1)),
        }
    }
    let shown = found.len().min(q.max.max(1));
    let mut out = format!(
        "{} matches for {:?} in result {id} ({} characters, {} lines); showing {shown}.",
        group(found.len()),
        q.pattern,
        group(text.chars().count()),
        group(text.lines().count())
    );
    for (start, end, first, count) in windows {
        let line = text[..first].matches('\n').count() + 1;
        let label = if count > 1 {
            format!(", {count} matches")
        } else {
            String::new()
        };
        let lead = if start > 0 { "…" } else { "" };
        let trail = if end < text.len() { "…" } else { "" };
        out.push_str(&format!(
            "\n\n--- line {line}{label}\n{lead}{}{trail}",
            &text[start..end]
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text() -> String {
        (1..=500)
            .map(|n| format!("row {n} value={}\n", n * 7))
            .collect()
    }

    #[test]
    fn reads_line_ranges() {
        let out = lines(&text(), "tr-1", Some(10), Some(12));
        assert_eq!(
            out,
            "Result tr-1: lines 10-12 of 500\n10\trow 10 value=70\n11\trow 11 value=77\n12\trow 12 value=84"
        );
        assert!(lines(&text(), "tr-1", None, None).ends_with("200\trow 200 value=1400"));
        assert!(lines(&text(), "tr-1", Some(499), Some(900)).ends_with("500\trow 500 value=3500"));
        assert_eq!(
            lines("a\nb", "tr-1", Some(5), None),
            "Result tr-1 has 2 lines; line 5 is past the end."
        );
    }

    #[test]
    fn searches_with_context() {
        let q = Search {
            pattern: "value=3500",
            regex: false,
            ignore_case: false,
            context: 8,
            max: 5,
        };
        let out = search(&text(), "tr-1", &q).unwrap();
        assert!(
            out.starts_with("1 matches for \"value=3500\" in result tr-1 ("),
            "{out}"
        );
        assert!(
            out.ends_with("--- line 500\n…row 500 value=3500\n"),
            "{out}"
        );
    }

    #[test]
    fn regex_search_merges_and_limits() {
        let q = Search {
            pattern: r"row 1\d ",
            regex: true,
            ignore_case: false,
            context: 30,
            max: 3,
        };
        let out = search(&text(), "tr-1", &q).unwrap();
        assert!(out.starts_with("10 matches for"), "{out}");
        assert!(out.contains("showing 3."), "{out}");
        assert!(out.contains("--- line 10, 3 matches\n"), "{out}");
        let bad = Search {
            pattern: "(",
            regex: true,
            ignore_case: false,
            context: 1,
            max: 1,
        };
        assert!(
            search("x", "tr-1", &bad)
                .unwrap_err()
                .starts_with("invalid pattern")
        );
        let none = Search {
            pattern: "zzz",
            regex: false,
            ignore_case: true,
            context: 1,
            max: 1,
        };
        assert_eq!(
            search("abc", "tr-1", &none).unwrap(),
            "No matches for \"zzz\" in result tr-1."
        );
    }

    #[test]
    fn search_handles_multibyte_text() {
        let q = Search {
            pattern: "ü",
            regex: false,
            ignore_case: false,
            context: 2,
            max: 9,
        };
        let out = search("ääüöö", "tr-1", &q).unwrap();
        assert!(out.ends_with("ääüöö"), "{out}");
    }
}
