//! Files and Project Knowledge sections.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::bash::command_core;
use crate::text::{Block, Kind, char_len, extract_path};

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("files regex")
}

static EDIT_TOOLS: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^(?:edit|multiedit|apply_patch|quick_edit|target_edit)$"));
static WRITE_TOOLS: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^(?:write|create)$"));
static READ_TOOLS: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^(?:read)$"));
static NOISE_READ: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)(?:^/tmp/|\.(?:png|jpe?g|gif|webp|bmp|svg|ico)$)"));
static NOTE: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)(?:^|/)(?:AGENTS?|CLAUDE|DESIGN|README|SKILL|CONTRIBUTING|RUNBOOK|NOTES)\.md$|(?:^|/)docs/.+\.md$|\.agents/skills/.+\.md$",
    )
});
static TOOLING: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"^(?:npm|npx|pnpm|yarn|bun|bunx|cargo|mise|make|just|go|pytest|uv|python3? -m|docker|docker-compose|kubectl|helm|rsync|scp|ssh|git push|gh|tea|systemctl|terraform|deno|flutter|xcodebuild)\b",
    )
});
static URL_HOST: LazyLock<Regex> = LazyLock::new(|| re(r"https?://([\w.-]+(?::\d+)?)"));
static SSH_HOST: LazyLock<Regex> =
    LazyLock::new(|| re(r"\b(?:ssh|scp|rsync)\b[^\n]*?\b([\w.-]+@[\w.-]+)"));
static SSH_START: LazyLock<Regex> = LazyLock::new(|| re(r"^ssh\b"));
static SSH_SPLIT: LazyLock<Regex> = LazyLock::new(|| re(r#"\s['"]"#));

/// `path` relative to `cwd` when it lies inside it.
pub fn relative(path: &str, cwd: Option<&str>) -> String {
    let (Some(cwd), true) = (cwd, Path::new(path).is_absolute()) else {
        return path.to_owned();
    };
    match Path::new(path).strip_prefix(cwd) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().into_owned(),
        _ => path.to_owned(),
    }
}

/// Items while they fit in `max_chars`, each counted with its `- ` line.
pub fn fit_items(items: &[String], max_chars: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut used = 0;
    for item in items {
        let cost = char_len(item) + 3;
        if used + cost > max_chars {
            break;
        }
        out.push(item.clone());
        used += cost;
    }
    out
}

fn list(label: &str, items: &[String], max_chars: usize) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let fitted = fit_items(items, max_chars);
    let more = items.len() - fitted.len();
    let suffix = if more > 0 {
        format!(" (+{more} more)")
    } else {
        String::new()
    };
    Some(format!("{label}: {}{suffix}", fitted.join(", ")))
}

fn push_unique(items: &mut Vec<String>, item: String) {
    if !items.contains(&item) {
        items.push(item);
    }
}

fn share(max_chars: usize, share: f64) -> usize {
    (max_chars as f64 * share).floor() as usize
}

pub fn select_files(blocks: &[Block], cwd: Option<&str>, max_chars: usize) -> Vec<String> {
    let mut modified: Vec<(String, usize)> = Vec::new();
    let mut created: Vec<String> = Vec::new();
    let mut read: Vec<String> = Vec::new();
    for block in blocks.iter().filter(|b| b.kind == Kind::ToolCall) {
        let Some(raw) = extract_path(&block.args) else {
            continue;
        };
        let path = relative(raw, cwd);
        if EDIT_TOOLS.is_match(&block.name) {
            match modified.iter_mut().find(|(p, _)| *p == path) {
                Some((_, n)) => *n += 1,
                None => modified.push((path, 1)),
            }
        } else if WRITE_TOOLS.is_match(&block.name) {
            push_unique(&mut created, path);
        } else if READ_TOOLS.is_match(&block.name) && !NOISE_READ.is_match(&path) {
            push_unique(&mut read, path);
        }
    }
    let is_modified = |p: &String| modified.iter().any(|(m, _)| m == p);
    created.retain(|p| !is_modified(p));
    read.retain(|p| !is_modified(p) && !created.contains(p));
    modified.sort_by_key(|m| std::cmp::Reverse(m.1));
    let by_count: Vec<String> = modified
        .iter()
        .map(|(p, n)| {
            if *n > 1 {
                format!("{p} ×{n}")
            } else {
                p.clone()
            }
        })
        .collect();
    read.reverse();
    [
        list("Modified", &by_count, share(max_chars, 0.45)),
        list("Created", &created, share(max_chars, 0.3)),
        list("Read", &read, share(max_chars, 0.2)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn balanced_quotes(text: &str) -> bool {
    text.chars().filter(|c| *c == '\'' || *c == '"').count() % 2 == 0
}

pub fn select_knowledge(blocks: &[Block], cwd: Option<&str>, max_chars: usize) -> Vec<String> {
    let mut edited: Vec<String> = Vec::new();
    let mut read: Vec<String> = Vec::new();
    let mut commands: Vec<(String, String, usize)> = Vec::new();
    let mut hosts: HashMap<String, (usize, usize)> = HashMap::new();
    let mut add_host = |host: &str| {
        let order = hosts.len();
        hosts.entry(host.to_owned()).or_insert((0, order)).0 += 1;
    };
    for block in blocks.iter().filter(|b| b.kind == Kind::ToolCall) {
        if let Some(path) = extract_path(&block.args).filter(|p| NOTE.is_match(p)) {
            let target = if EDIT_TOOLS.is_match(&block.name) || WRITE_TOOLS.is_match(&block.name) {
                &mut edited
            } else {
                &mut read
            };
            push_unique(target, relative(path, cwd));
        }
        let Some(command) = block.arg_str("command").filter(|c| !c.is_empty()) else {
            continue;
        };
        let mut core = command_core(command);
        if SSH_START.is_match(&core) {
            core = SSH_SPLIT
                .split(&core)
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned();
        }
        if TOOLING.is_match(&core)
            && char_len(&core) <= 160
            && !core.contains("<<")
            && !core.ends_with('\\')
            && balanced_quotes(&core)
        {
            let key = core
                .split_whitespace()
                .take(3)
                .collect::<Vec<_>>()
                .join(" ");
            match commands.iter_mut().find(|(k, _, _)| *k == key) {
                Some(entry) => {
                    entry.1 = core;
                    entry.2 += 1;
                }
                None => commands.push((key, core, 1)),
            }
        }
        for caps in URL_HOST.captures_iter(command) {
            add_host(&caps[1]);
        }
        for caps in SSH_HOST.captures_iter(command) {
            add_host(&caps[1]);
        }
    }
    read.retain(|p| !edited.contains(p));
    let mut lines = Vec::new();
    let recent_reads: Vec<String> = read[read.len().saturating_sub(8)..].to_vec();
    commands.sort_by_key(|c| std::cmp::Reverse(c.2));
    let repeated: Vec<String> = commands
        .iter()
        .filter(|c| c.2 >= 2)
        .take(8)
        .map(|(_, text, n)| format!("`{text}` ×{n}"))
        .collect();
    let mut top_hosts: Vec<(&String, &(usize, usize))> =
        hosts.iter().filter(|(_, (n, _))| *n >= 2).collect();
    top_hosts.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.1.1.cmp(&b.1.1)));
    let top_hosts: Vec<String> = top_hosts
        .into_iter()
        .take(6)
        .map(|(h, _)| h.clone())
        .collect();
    for (label, items, part) in [
        ("Notes written", edited, 0.2),
        ("Notes read", recent_reads, 0.15),
        ("Repeated commands", repeated, 0.45),
        ("Hosts", top_hosts, 0.2),
    ] {
        if let Some(line) = list(label, &items, share(max_chars, part))
            .filter(|_| !fit_items(&items, share(max_chars, part)).is_empty())
        {
            lines.push(line);
        }
    }
    lines
}
