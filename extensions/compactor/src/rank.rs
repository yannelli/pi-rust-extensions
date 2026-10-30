//! Ranked block selection for the transcript: score by signal, fill a char budget.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::bash::is_trivial_only_bash;
use crate::brief::compile_brief;
use crate::text::{Block, Kind, extract_path, one_line};

const DEFAULT_MAX_BLOCKS: usize = 80;
const DEFAULT_RECENT_BLOCKS: usize = 16;
const MIN_SEGMENT_CLOSING_CHARS: usize = 120;
const TRIVIAL_BASH_PENALTY: i64 = 16;

static EDIT_TOOL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(edit|write|multiedit|quick_edit|target_edit|apply_patch)$").unwrap()
});
static READ_TOOL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(read|glob|grep|ls|find|semantic_query|semantic_grep|semantic_show)$")
        .unwrap()
});
static TEST_COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:bun|npm|pnpm|yarn|node|pytest|cargo|go|mvn|gradle)\b[^\n]*(?:test|spec|check|lint|build|tsc)").unwrap()
});
static GH_PR_POLL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|\s)gh\s+pr\s+(?:view|checks)\s+(\d+)\b").unwrap());
static WORKFLOW_COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|\s)(?:gh\s+(?:pr|issue)\s+[a-z-]+|git\s+(?:commit|push|merge|rebase|revert|cherry-pick|tag|reset|checkout|branch)\b)")
        .unwrap()
});

/// Hook-provided file activity, a structural ranking signal.
#[derive(Debug, Clone, Default)]
pub struct FileOps {
    pub read: Vec<String>,
    pub modified: Vec<String>,
}

#[derive(Debug, Clone)]
struct Ranked {
    score: i64,
    important: bool,
}

fn bash_command(block: &Block) -> Option<&str> {
    (block.kind == Kind::ToolCall && block.name.eq_ignore_ascii_case("bash"))
        .then(|| block.arg_str("command"))
        .flatten()
}

fn score(block: &Block, index: usize, total: usize, ops: &FileOps) -> Ranked {
    let mut score = if total <= 1 {
        0
    } else {
        ((index as f64 / (total - 1) as f64) * 12.0).round() as i64
    };
    let mut important = false;
    match block.kind {
        Kind::User => score += 18,
        Kind::Assistant => score += 10,
        Kind::ToolResult => score += 1,
        _ => {}
    }
    if block.kind == Kind::ToolCall {
        let command = bash_command(block);
        if EDIT_TOOL.is_match(&block.name) {
            score += 34;
            important = true;
        } else if command.is_some_and(|c| TEST_COMMAND.is_match(c)) {
            score += 26;
            important = true;
        } else if READ_TOOL.is_match(&block.name) {
            score += 6;
        } else {
            score += 12;
        }
        if command.is_some_and(|c| WORKFLOW_COMMAND.is_match(c)) {
            score += 14;
        }
        if command.is_some_and(is_trivial_only_bash) {
            score -= TRIVIAL_BASH_PENALTY;
        }
        if let Some(path) = extract_path(&block.args) {
            if ops.modified.iter().any(|p| p == path) {
                score += 18;
            }
            if ops.read.iter().any(|p| p == path) {
                score += 6;
            }
        }
    }
    if block.kind == Kind::ToolResult && block.text.chars().count() > 1000 {
        score -= 8;
    }
    Ranked { score, important }
}

fn boost(ranked: &mut [Ranked], blocks: &[Block]) {
    let important: Vec<usize> = (0..ranked.len())
        .filter(|i| ranked[*i].score >= 34 || ranked[*i].important)
        .collect();
    for idx in important {
        if let Some(i) = (idx.saturating_sub(8)..idx)
            .rev()
            .find(|i| blocks[*i].kind == Kind::User)
        {
            ranked[i].score += 10;
        }
        if let Some(i) = (idx.saturating_sub(4)..idx)
            .rev()
            .find(|i| blocks[*i].kind == Kind::Assistant)
        {
            ranked[i].score += 7;
        }
        let after = (idx + 1..=(idx + 4).min(ranked.len().saturating_sub(1)))
            .find(|i| matches!(blocks[*i].kind, Kind::Assistant | Kind::ToolCall));
        if let Some(i) = after {
            ranked[i].score += 5;
        }
    }
    for i in 0..blocks.len() {
        if blocks[i].kind != Kind::Assistant
            || blocks[i].text.trim().chars().count() < MIN_SEGMENT_CLOSING_CHARS
        {
            continue;
        }
        let next = blocks[i + 1..].iter().find(|b| b.kind != Kind::ToolResult);
        if next.is_none_or(|b| b.kind == Kind::User) {
            ranked[i].score += 14;
        }
    }
}

fn dedup_key(block: &Block) -> Option<String> {
    if let Some(command) = bash_command(block) {
        if let Some(caps) = GH_PR_POLL.captures(command) {
            return Some(format!("gh-pr-poll:{}", &caps[1]));
        }
        let normalized = one_line(command);
        return (!normalized.is_empty()).then(|| format!("bash:{normalized}"));
    }
    if block.kind == Kind::ToolCall {
        return extract_path(&block.args)
            .map(|p| format!("tool:{}:{p}", block.name.to_lowercase()));
    }
    None
}

/// The highest-value blocks whose rendered brief fits `max_chars`, in session order.
pub fn select_ranked(blocks: &[Block], max_chars: usize, ops: &FileOps) -> Vec<Block> {
    let mut ranked: Vec<Ranked> = blocks
        .iter()
        .enumerate()
        .map(|(i, b)| score(b, i, blocks.len(), ops))
        .collect();
    boost(&mut ranked, blocks);
    let costs: Vec<usize> = blocks
        .iter()
        .map(|b| {
            if b.kind == Kind::ToolResult {
                0
            } else {
                compile_brief(std::slice::from_ref(b)).chars().count() + 1
            }
        })
        .collect();
    let mut selected: HashSet<usize> = HashSet::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut used = 0;
    let recent_floor = blocks
        .len()
        .saturating_sub(DEFAULT_RECENT_BLOCKS.min(DEFAULT_MAX_BLOCKS));
    for i in (recent_floor..blocks.len()).rev() {
        if blocks[i].kind == Kind::ToolResult || used + costs[i] > max_chars {
            continue;
        }
        selected.insert(i);
        used += costs[i];
        seen.extend(dedup_key(&blocks[i]));
    }
    let mut order: Vec<usize> = (0..blocks.len()).collect();
    order.sort_by(|a, b| ranked[*b].score.cmp(&ranked[*a].score).then(b.cmp(a)));
    for i in order {
        if selected.len() >= DEFAULT_MAX_BLOCKS {
            break;
        }
        if selected.contains(&i) || blocks[i].kind == Kind::ToolResult {
            continue;
        }
        let key = dedup_key(&blocks[i]);
        if key.as_ref().is_some_and(|k| seen.contains(k)) || used + costs[i] > max_chars {
            continue;
        }
        used += costs[i];
        selected.insert(i);
        seen.extend(key);
    }
    let mut indices: Vec<usize> = selected.into_iter().collect();
    indices.sort_unstable();
    indices.into_iter().map(|i| blocks[i].clone()).collect()
}
