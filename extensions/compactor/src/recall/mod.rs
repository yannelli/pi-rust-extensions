//! `session_recall`: search, paging, expand, touched files, and drill-down.

pub mod drill;
pub mod search;

use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

use crate::text::{Block, Kind, extract_path};
use search::{Hit, search};

pub const PAGE_SIZE: usize = 5;
pub const DEFAULT_RECENT: usize = 25;
pub const MAX_EXPANDED_HIT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RecallInput {
    pub query: Option<String>,
    pub expand: Option<Vec<f64>>,
    pub page: Option<f64>,
    pub scope: Option<String>,
    pub mode: Option<String>,
}

pub fn scope_is_all(scope: Option<&str>) -> bool {
    scope.is_some_and(|s| s.eq_ignore_ascii_case("all"))
}

static SCOPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bscope:(lineage|all)\b").unwrap());
static PAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bpage:(\d+)\b").unwrap());

/// `/unipi:session-recall` arguments: `(query, all scope, page)`.
pub fn parse_command_args(raw: &str) -> (String, bool, usize) {
    let all = SCOPE
        .captures(raw)
        .is_some_and(|c| c[1].eq_ignore_ascii_case("all"));
    let text = crate::text::one_line(&SCOPE.replace(raw, ""));
    let page = PAGE
        .captures(&text)
        .and_then(|c| c[1].parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let query = PAGE.replace(&text, "").trim().to_owned();
    (query, all, page)
}

fn short_path(path: &str, cwd: &str) -> String {
    let normalized = path.replace('\\', "/");
    let base = cwd.replace('\\', "/");
    if !base.is_empty()
        && let Some(rest) = normalized.strip_prefix(&format!("{base}/"))
    {
        return format!("./{rest}");
    }
    let parts: Vec<&str> = normalized.split('/').collect();
    if parts.len() > 3 {
        format!(".../{}", parts[parts.len() - 3..].join("/"))
    } else {
        normalized
    }
}

fn touched_output(blocks: &[Block], page: usize, cwd: &str) -> String {
    let mut touched: Vec<(String, Vec<String>)> = Vec::new();
    for block in blocks.iter().filter(|b| b.kind == Kind::ToolCall) {
        let Some(path) = extract_path(&block.args) else {
            continue;
        };
        let entry = format!("#{} ({})", block.source.unwrap_or(0), block.name);
        match touched.iter_mut().find(|(p, _)| p == path) {
            Some((_, entries)) => entries.push(entry),
            None => touched.push((path.to_owned(), vec![entry])),
        }
    }
    if touched.is_empty() {
        return "No file operations found in session history.".into();
    }
    let total_pages = touched.len().div_ceil(PAGE_SIZE);
    let start = (page - 1) * PAGE_SIZE;
    let header = if total_pages > 1 {
        format!("Page {page}/{total_pages} ({} total files)", touched.len())
    } else {
        format!("{} files touched", touched.len())
    };
    let lines: Vec<String> = touched
        .iter()
        .skip(start)
        .take(PAGE_SIZE)
        .map(|(path, entries)| format!("  {}    {}", short_path(path, cwd), entries.join(", ")))
        .collect();
    let mut out = format!("{header}:\n\n{}", lines.join("\n"));
    if page < total_pages {
        out.push_str(&format!(
            "\n\n--- Use page:{} for more results ---",
            page + 1
        ));
    }
    out
}

fn format_hits(hits: &[Hit], query: Option<&str>, header: Option<&str>) -> String {
    if hits.is_empty() {
        return match query {
            Some(q) => format!("No matches for \"{q}\" in session history."),
            None => "No entries in session history.".into(),
        };
    }
    let header = match (header, query) {
        (Some(h), Some(q)) => format!("{h} for \"{q}\":"),
        (_, Some(q)) => format!("Found {} matches for \"{q}\":", hits.len()),
        _ => format!("Session history ({} entries):", hits.len()),
    };
    let lines: Vec<String> = hits
        .iter()
        .map(|h| {
            let files = if h.files.is_empty() {
                String::new()
            } else {
                format!(" files:[{}]", h.files.join(", "))
            };
            format!(
                "#{} [{}]{files} {}",
                h.index,
                h.kind,
                h.snippet.as_deref().unwrap_or(&h.text)
            )
        })
        .collect();
    format!("{header}\n\n{}", lines.join("\n\n"))
}

fn truncate_hit(text: String) -> String {
    if text.len() <= MAX_EXPANDED_HIT_BYTES {
        return text;
    }
    let mut cut = MAX_EXPANDED_HIT_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let omitted = text.len() - MAX_EXPANDED_HIT_BYTES;
    format!(
        "{}\n… {omitted} bytes omitted from this hit; narrow the query to inspect more specific context …",
        &text[..cut]
    )
}

fn expand(blocks: &[Block], requested: &[f64], all: bool) -> String {
    let available = |i: usize| blocks.iter().any(|b| b.source == Some(i));
    let invalid: Vec<String> = requested
        .iter()
        .filter(|i| i.fract() != 0.0 || **i < 0.0 || !available(**i as usize))
        .map(|i| i.to_string())
        .collect();
    if !invalid.is_empty() {
        let within = if all {
            "session history"
        } else {
            "active lineage"
        };
        return format!(
            "Cannot expand indices outside {within}: {}",
            invalid.join(", ")
        );
    }
    let mut indices: Vec<usize> = requested.iter().map(|i| *i as usize).collect();
    indices.sort_unstable();
    indices.dedup();
    let lines: Vec<String> = indices
        .iter()
        .flat_map(|i| blocks.iter().filter(move |b| b.source == Some(*i)))
        .map(|b| {
            let text = if b.kind == Kind::ToolCall {
                format!("{} {}", b.name, b.args)
            } else {
                b.text.clone()
            };
            format!(
                "#{} [{}] {}",
                b.source.unwrap_or(0),
                b.kind.as_str(),
                truncate_hit(text)
            )
        })
        .collect();
    format!(
        "{}{}",
        if all { "Scope: all\n\n" } else { "" },
        lines.join("\n\n")
    )
}

pub fn vcc_recall(blocks: &[Block], input: &RecallInput, cwd: &str) -> String {
    let all = scope_is_all(input.scope.as_deref());
    let query = input
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty());
    let page = input.page.map_or(1, |p| p.max(1.0) as usize);
    if let Some(drill) = query.and_then(drill::parse_drill) {
        return drill::expand_entry_file(blocks, &drill);
    }
    if input
        .mode
        .as_deref()
        .is_some_and(|m| m.eq_ignore_ascii_case("touched"))
    {
        return touched_output(blocks, page, cwd);
    }
    let requested = input.expand.clone().unwrap_or_default();
    if !requested.is_empty() && query.is_none() {
        return expand(blocks, &requested, all);
    }
    let scope_suffix = if all { " (scope: all)" } else { "" };
    let Some(q) = query else {
        let hits = search(blocks, None);
        let recent = &hits[hits.len().saturating_sub(DEFAULT_RECENT)..];
        return format!(
            "{}{}",
            if all { "Scope: all\n\n" } else { "" },
            format_hits(recent, None, None)
        );
    };
    let hits = search(blocks, Some(q));
    let total_pages = hits.len().div_ceil(PAGE_SIZE);
    let page_hits: Vec<Hit> = hits
        .iter()
        .skip((page - 1) * PAGE_SIZE)
        .take(PAGE_SIZE)
        .cloned()
        .collect();
    if page_hits.is_empty() {
        let on_page = if page > 1 {
            format!(" on page {page}")
        } else {
            String::new()
        };
        return format!("No matches for \"{q}\"{scope_suffix}{on_page}.");
    }
    let header = if total_pages > 1 {
        format!(
            "Page {page}/{total_pages} ({} total matches{scope_suffix})",
            hits.len()
        )
    } else {
        format!("{} matches{scope_suffix}", hits.len())
    };
    let footer = if page < total_pages {
        format!(
            "\n--- Use page:{}{} for more results ---",
            page + 1,
            if all { " with scope:'all'" } else { "" }
        )
    } else {
        String::new()
    };
    format!(
        "{}{footer}",
        format_hits(&page_hits, Some(q), Some(&header))
    )
}
