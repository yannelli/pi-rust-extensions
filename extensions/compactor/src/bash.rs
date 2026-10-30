//! Shell command shapes: heredoc detection, one-line compression, trivial scripts.

use std::sync::LazyLock;

use regex::Regex;

use crate::text::ELLIPSIS;

static HEREDOC_OPEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<<-?\s*["']?([A-Za-z_]\w*)["']?"#).unwrap());
static FILEWRITER_HEREDOC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[|&;]\s*)(?:cat|tee|dd)\b").unwrap());
static BODY_NOISE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^(?:import\s|from\s+\S+\s+import|require\(|const\s+\w+\s*=\s*require|#|//|"""|'''|"use strict"|use\s+strict|<\?php)"#)
        .unwrap()
});
static TRIVIAL_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:set\s+[-+]|cd\s+\S+$|export\s+\w+=|(?:source|\.)\s+\S+|pwd$|true$|:$|#)")
        .unwrap()
});
static TRIVIAL_BASH_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:set\s+[-+]|cd(?:\s+\S+)?$|export\s+\w+=|(?:source|\.)\s+\S+|pwd$|true$|:$|#|ls(?:\s|$)|echo\b|clear$|sleep\b)")
        .unwrap()
});
static CD_PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^cd\s+\S+\s*&&\s*").unwrap());
static PIPE_TAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\s*\|\s*(?:head|tail|sort|wc|column|tr|cut|awk|uniq|python3|node|bun)(?:\s[^|]*)?$",
    )
    .unwrap()
});

const BASH_CAP: usize = 240;
const HEREDOC_BODY_CAP: usize = 80;

/// The heredoc terminator of `line`: `<<` at a command boundary (no word char or `)` before it).
fn heredoc_terminator(line: &str) -> Option<&str> {
    HEREDOC_OPEN.captures_iter(line).find_map(|caps| {
        let start = caps.get(0)?.start();
        let before = line[..start].chars().next_back();
        let boundary = before.is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == ')'));
        boundary.then(|| caps.get(1).map(|m| m.as_str())).flatten()
    })
}

/// Index of the line that closes a heredoc opened on `lines[i]`, when one exists.
pub fn heredoc_close_index(lines: &[&str], i: usize) -> Option<usize> {
    let term = heredoc_terminator(lines[i])?;
    (i + 1..lines.len()).find(|j| lines[*j].trim() == term)
}

fn strip_pipe_tail(line: &str) -> String {
    let mut current = line.to_owned();
    for _ in 0..3 {
        let stripped = PIPE_TAIL.replace(&current, "").into_owned();
        if stripped == current {
            break;
        }
        current = stripped;
    }
    current.trim().to_owned()
}

fn strip_cd(line: &str) -> String {
    CD_PREFIX.replace(line, "").trim().to_owned()
}

/// One line for a shell script: heredoc bodies dropped, preamble dropped, capped at 240 chars.
pub fn compress_bash(raw: &str) -> String {
    let raw_lines: Vec<&str> = raw.split('\n').collect();
    let mut kept: Vec<String> = Vec::new();
    let mut i = 0;
    while i < raw_lines.len() {
        let line = raw_lines[i];
        kept.push(line.to_owned());
        if let Some(close) = heredoc_close_index(&raw_lines, i) {
            if !FILEWRITER_HEREDOC.is_match(line) {
                let preview = raw_lines[i + 1..close]
                    .iter()
                    .map(|l| l.trim())
                    .find(|t| !t.is_empty() && !BODY_NOISE.is_match(t));
                if let Some(preview) = preview {
                    let clipped = if preview.chars().count() > HEREDOC_BODY_CAP {
                        let head: String = preview.chars().take(HEREDOC_BODY_CAP - 1).collect();
                        format!("{head}{ELLIPSIS}")
                    } else {
                        preview.to_owned()
                    };
                    *kept.last_mut().expect("pushed") = format!("{} {clipped}", line.trim());
                }
            }
            i = close;
        }
        i += 1;
    }
    let lines: Vec<&str> = kept
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return raw.trim().to_owned();
    }
    let meaningful: Vec<String> = lines
        .iter()
        .filter(|l| !TRIVIAL_LINE.is_match(l))
        .map(|l| strip_pipe_tail(&strip_cd(l)))
        .filter(|l| !l.is_empty())
        .collect();
    let chosen = if meaningful.is_empty() {
        vec![strip_pipe_tail(&strip_cd(lines[0]))]
            .into_iter()
            .filter(|l| !l.is_empty())
            .collect()
    } else {
        meaningful
    };
    let cmd = chosen.join("; ");
    if cmd.chars().count() > BASH_CAP {
        let head: String = cmd.chars().take(BASH_CAP - 3).collect();
        format!("{head}...")
    } else {
        cmd
    }
}

/// Every meaningful line is scaffolding (set, cd, export, ls, echo, sleep, comments).
pub fn is_trivial_only_bash(raw: &str) -> bool {
    let lines: Vec<&str> = raw.split('\n').collect();
    let mut kept = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        kept.push(lines[i]);
        if let Some(close) = heredoc_close_index(&lines, i) {
            i = close;
        }
        i += 1;
    }
    kept.iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .all(|l| TRIVIAL_BASH_LINE.is_match(l))
}

static ENV_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:[A-Z_][A-Z0-9_]*=\S+\s+)+").unwrap());
static REDIRECT_TAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*(?:\||2>&1|>\s*\S+|>>\s*\S+).*$").unwrap());
static SEGMENT_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*(?:&&|;)\s*").unwrap());
static SETUP_CMD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:cd|export|source|set|echo)\b").unwrap());

/// The first meaningful command of a script: no `cd x &&`, pipes, redirects, or env prefixes.
pub fn command_core(command: &str) -> String {
    let first = command
        .split('\n')
        .find(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
        .unwrap_or_default();
    let main = SEGMENT_SPLIT
        .split(first)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .find(|s| !SETUP_CMD.is_match(s))
        .unwrap_or_default();
    let without_env = ENV_PREFIX.replace(main, "");
    REDIRECT_TAIL.replace(&without_env, "").trim().to_owned()
}
