//! Your Requests, Decisions & Constraints, and Latest State.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::text::{char_len, clip, one_line};

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("summary regex")
}

static TRIVIAL_REQUEST: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)^(?:continue|go on|go ahead|carry on|keep going|proceed|next|go|ok(?:ay)?|yes|yep|y|sure|thanks?|thank you|ty|k|done|resume|restarted[^.]*)[\s.!~]*$",
    )
});
static IMPERATIVE_START: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)^(?:(?:please|also|and|then|but|so)[, ]+)?(?:don'?t|do not|never|always|keep|avoid|make sure|ensure|use|prefer|remember|only use|stop|no need|restore|revert|undo|preserve|stick to|stay with|go back to)\b",
    )
});
static MODAL: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\b(?:should(?:n'?t)?|must|prefer|instead of|instead|rather than|required|important|no need)\b",
    )
});
static CORRECTION: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\bI (?:did not|didn't|never) (?:ask|request|want|say)\b|\bnot what I (?:asked|wanted|meant)\b|\bwhy (?:did|do) you (?:change|remove|delete)\b",
    )
});
static PASTED_AI: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\bI(?:'m| am) an AI\b|\bas an AI\b|\bI can help (?:with|you)\b|\bif you'd like, I\b|\bI can'?t do that\b|\*\*[^*\n]+\*\*",
    )
});
static STANDING: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)\b(?:always|never|every|all (?:the )?\w+|by default|default|from now on|going forward|in general|globally|whenever|each time|unless|any time)\b",
    )
});
static FENCE: LazyLock<Regex> = LazyLock::new(|| re(r"```[\s\S]*?(?:```|$)"));
static QUOTE_LINE: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^\s*>.*$"));
static SENTENCE_END: LazyLock<Regex> = LazyLock::new(|| re(r#"[.!?]\s+[A-Z0-9"'`]"#));
static BULLET: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*(?:[-*+>]|\d+[.)])\s+"));
static QUESTION_END: LazyLock<Regex> = LazyLock::new(|| re(r#"\?["')\]]*$"#));
static NEEDS_CONTEXT: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^(?:it|this|that|these|those)\b"));
static CODE_LIKE: LazyLock<Regex> = LazyLock::new(|| re(r"^[`$]|[{};]\s*$"));
static IMAGE_MD: LazyLock<Regex> = LazyLock::new(|| re(r"!\[[^\]]*\]\([^)]*\)"));

pub fn is_trivial(text: &str) -> bool {
    TRIVIAL_REQUEST.is_match(text)
}

/// Splits after `.`, `!`, or `?` plus whitespace when the next sentence opens with a capital, digit, or quote.
pub fn split_sentences(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for m in SENTENCE_END.find_iter(line) {
        out.push(&line[start..m.start() + 1]);
        start = m.end()
            - line[..m.end()]
                .chars()
                .next_back()
                .map_or(1, char::len_utf8);
    }
    out.push(&line[start..]);
    out
}

#[derive(Debug, Clone, Default)]
pub struct RequestSelection {
    pub lines: Vec<String>,
    pub shown: HashSet<usize>,
}

/// The opening request and the newest ones that fit, oldest first.
pub fn select_request_lines(requests: &[String], max_chars: usize) -> RequestSelection {
    let all: Vec<(String, usize)> = requests
        .iter()
        .enumerate()
        .map(|(i, r)| (one_line(r), i))
        .filter(|(text, _)| !text.is_empty() && !is_trivial(text))
        .collect();
    let mut selection = RequestSelection::default();
    let Some(((first, first_i), rest)) = all.split_first() else {
        return selection;
    };
    let per_item = (max_chars / all.len().min(6)).saturating_sub(4).max(160);
    let opening = clip(first, 240);
    selection.shown.insert(*first_i);
    let mut budget = max_chars as i64 - char_len(&opening) as i64 - 3;
    let mut recent: Vec<String> = Vec::new();
    for (text, i) in rest.iter().rev() {
        if recent.len() >= 5 {
            break;
        }
        let item = clip(text, per_item);
        let cost = char_len(&item) as i64 + 3;
        if cost > budget {
            break;
        }
        budget -= cost;
        selection.shown.insert(*i);
        recent.insert(0, item);
    }
    selection.lines.push(opening);
    let omitted = rest.len() - recent.len();
    if omitted > 0 {
        let plural = if omitted == 1 { "" } else { "s" };
        selection.lines.push(format!(
            "({omitted} more request{plural} in between — session_recall has them)"
        ));
    }
    selection.lines.extend(recent);
    selection
}

fn is_decision(piece: &str) -> bool {
    IMPERATIVE_START.is_match(piece) || MODAL.is_match(piece) || CORRECTION.is_match(piece)
}

fn key60(text: &str) -> String {
    text.to_lowercase().chars().take(60).collect()
}

/// Instructions and corrections from the user's requests, recent ones and standing rules first.
pub fn select_decisions(
    requests: &[String],
    max_chars: usize,
    skip: &HashSet<usize>,
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut found: Vec<String> = Vec::new();
    for (idx, request) in requests.iter().enumerate() {
        if skip.contains(&idx) || is_trivial(&one_line(request)) {
            continue;
        }
        if request.contains(" → ") {
            if seen.insert(key60(request)) {
                found.push(clip(&one_line(request), 260));
            }
            continue;
        }
        let stripped = FENCE.replace_all(request, "\n");
        let stripped = QUOTE_LINE.replace_all(&stripped, "");
        let pieces: Vec<String> = stripped
            .split('\n')
            .filter(|l| !l.is_empty())
            .flat_map(split_sentences)
            .map(|p| BULLET.replace(p, "").trim().to_owned())
            .filter(|p| (24..=400).contains(&char_len(p)))
            .collect();
        let mut pushed: Option<usize> = None;
        for piece in pieces {
            if !is_decision(&piece) {
                continue;
            }
            if let Some(at) = pushed
                && CORRECTION.is_match(&piece)
                && !IMPERATIVE_START.is_match(&piece)
                && !MODAL.is_match(&piece)
            {
                found[at] = clip(&format!("{} {}", found[at], one_line(&piece)), 260);
                continue;
            }
            if QUESTION_END.is_match(&piece)
                || PASTED_AI.is_match(&piece)
                || CODE_LIKE.is_match(&piece)
            {
                continue;
            }
            if NEEDS_CONTEXT.is_match(&piece) && char_len(&piece) < 60 {
                continue;
            }
            if !seen.insert(key60(&piece)) {
                continue;
            }
            found.push(clip(&one_line(&piece), 220));
            pushed = Some(found.len() - 1);
        }
    }
    pick_decisions(&found, max_chars)
}

fn pick_decisions(found: &[String], max_chars: usize) -> Vec<String> {
    let mut chosen: Vec<usize> = Vec::new();
    let mut used = 0;
    let mut take = |i: usize, chosen: &mut Vec<usize>| {
        let cost = char_len(&found[i]) + 3;
        if !chosen.contains(&i) && used + cost <= max_chars {
            chosen.push(i);
            used += cost;
        }
        used
    };
    let mut current = 0;
    for i in (0..found.len()).rev() {
        if current * 2 >= max_chars {
            break;
        }
        current = take(i, &mut chosen);
    }
    for (i, item) in found.iter().enumerate() {
        if STANDING.is_match(item) || CORRECTION.is_match(item) {
            take(i, &mut chosen);
        }
    }
    for i in (0..found.len()).rev() {
        take(i, &mut chosen);
    }
    chosen.sort_unstable();
    chosen.into_iter().map(|i| found[i].clone()).collect()
}

const REPORT_MIN_CHARS: usize = 400;

/// The latest progress report, plus the latest step when the report is older.
pub fn select_state(reports: &[String], max_chars: usize) -> Vec<String> {
    let flat: Vec<String> = reports
        .iter()
        .map(|r| one_line(&IMAGE_MD.replace_all(r, "")))
        .filter(|r| !r.is_empty())
        .collect();
    let Some(latest) = flat.len().checked_sub(1) else {
        return Vec::new();
    };
    let report = (0..flat.len())
        .rev()
        .find(|i| char_len(&flat[*i]) >= REPORT_MIN_CHARS);
    let step_cap = (max_chars as f64 * 0.3).floor() as usize;
    match report {
        Some(r) if r == latest => vec![format!(
            "Last report: {}",
            clip(&flat[r], max_chars.saturating_sub(20))
        )],
        None => vec![format!("Latest step: {}", clip(&flat[latest], step_cap))],
        Some(r) => {
            let step = format!("Latest step: {}", clip(&flat[latest], step_cap));
            let age = latest - r;
            let plural = if age == 1 { "" } else { "s" };
            let label =
                format!("Last full report ({age} message{plural} earlier — may be outdated)");
            let room = max_chars.saturating_sub(char_len(&step) + char_len(&label) + 10);
            vec![step.clone(), format!("{label}: {}", clip(&flat[r], room))]
        }
    }
}
