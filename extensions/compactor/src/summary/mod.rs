//! Lossless (zero-LLM) summary: state first, each section on its own share of the budget.

pub mod commits;
pub mod files;
pub mod history;
pub mod requests;

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::config::Sections;
use crate::rank::FileOps;
use crate::source::SummarySource;
use crate::text::{Kind, char_len, clip};

pub const RECALL_NOTE: &str = "Everything above is condensed from the full session history, which is kept. \
Use `session_recall` to search it (refs like #123 are entry indices; pass them in `expand` for full content). \
Do not redo work that is already done.";

static SECRETS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let words = r"(password|passwd|passphrase|pwd|token|secret|api[_ -]?key)";
    let bare = r#"[^\s'"`]{4,}"#;
    let quoted = r#"[^'"`\n]{4,}?"#;
    [
        (
            format!(r#"(?i)\b{words}(\s*(?:is|=|:)\s*)(?:"{bare}"|'{bare}'|`{bare}`|{bare})"#),
            "${1}${2}[redacted]",
        ),
        (
            format!(r#"(?i)\b{words}(\s+)(?:"{quoted}"|'{quoted}'|`{quoted}`)"#),
            "${1}${2}[redacted]",
        ),
        (r"\bsk-[A-Za-z0-9_-]{16,}".to_owned(), "[redacted]"),
        (
            r"(?i)\b(Bearer|X-API-KEY:?|Authorization:)\s+[A-Za-z0-9._~+/=-]{12,}".to_owned(),
            "${1} [redacted]",
        ),
        (r"\bgh[pousr]_[A-Za-z0-9]{20,}".to_owned(), "[redacted]"),
        (r"\bAKIA[0-9A-Z]{16}\b".to_owned(), "[redacted]"),
        (r"\bxox[abprs]-[A-Za-z0-9-]{10,}".to_owned(), "[redacted]"),
        (
            r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}".to_owned(),
            "[redacted]",
        ),
        (
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?(?:-----END [A-Z ]*PRIVATE KEY-----|$)"
                .to_owned(),
            "[redacted private key]",
        ),
    ]
    .into_iter()
    .map(|(pattern, to)| (Regex::new(&pattern).expect("secret regex"), to))
    .collect()
});

/// Removes credentials from summary text; session_recall still sees the originals.
pub fn redact_secrets(text: &str) -> String {
    SECRETS.iter().fold(text.to_owned(), |text, (re, to)| {
        re.replace_all(&text, *to).into_owned()
    })
}

/// Auto budget in tokens: 1.5k to 4k, scaling with the amount of history.
pub fn auto_budget_tokens(block_count: usize) -> usize {
    (1500.0 + block_count as f64 * 8.0)
        .clamp(1500.0, 4000.0)
        .round() as usize
}

#[derive(Debug, Clone, Default)]
pub struct SummaryInput {
    pub source: SummarySource,
    pub active_work: Vec<(String, String)>,
    pub budget_chars: usize,
    pub cwd: Option<String>,
    pub file_ops: FileOps,
    pub sections: Sections,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    pub text: String,
    pub sections: Vec<String>,
}

fn section(title: &str, lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let body: Vec<String> = lines.iter().map(|l| format!("- {l}")).collect();
    format!("[{title}]\n{}", body.join("\n"))
}

fn redacted_source(source: &SummarySource) -> SummarySource {
    let mut out = source.clone();
    out.requests = out.requests.iter().map(|r| redact_secrets(r)).collect();
    out.reports = out.reports.iter().map(|r| redact_secrets(r)).collect();
    for block in out.blocks.iter_mut().filter(|b| b.kind != Kind::ToolCall) {
        block.text = redact_secrets(&block.text);
    }
    out
}

pub fn build_lossless_summary(input: &SummaryInput) -> Summary {
    let source = redacted_source(&input.source);
    let on = &input.sections;
    let budget = input.budget_chars.max(2000);
    let cap = |share: f64| (budget as f64 * share).floor() as usize;
    let cwd = input.cwd.as_deref();
    let mut parts: Vec<(&str, String)> = Vec::new();

    if on.active_work && !input.active_work.is_empty() {
        let per = (cap(0.16) / input.active_work.len()).max(200);
        let lines: Vec<String> = input
            .active_work
            .iter()
            .map(|(_, text)| clip(text.trim(), per))
            .collect();
        parts.push((
            "Active Work",
            format!("[Active Work]\n{}", lines.join("\n\n")),
        ));
    }
    let selection = if on.requests {
        requests::select_request_lines(&source.requests, cap(0.14))
    } else {
        requests::RequestSelection {
            lines: Vec::new(),
            shown: HashSet::new(),
        }
    };
    if on.requests {
        parts.push(("Your Requests", section("Your Requests", &selection.lines)));
    }
    if on.state {
        parts.push((
            "Latest State",
            section(
                "Latest State",
                &requests::select_state(&source.reports, cap(0.12)),
            ),
        ));
    }
    if on.decisions {
        let decisions = requests::select_decisions(&source.requests, cap(0.16), &selection.shown);
        parts.push((
            "Decisions & Constraints",
            section("Decisions & Constraints", &decisions),
        ));
    }
    if on.files {
        parts.push((
            "Files",
            section(
                "Files",
                &files::select_files(&source.blocks, cwd, cap(0.07)),
            ),
        ));
    }
    if on.lessons {
        parts.push((
            "Lessons",
            section(
                "Lessons",
                &history::select_lessons(&source.blocks, cap(0.1)),
            ),
        ));
    }
    if on.files {
        let knowledge = files::select_knowledge(&source.blocks, cwd, cap(0.07));
        parts.push((
            "Project Knowledge",
            section("Project Knowledge", &knowledge),
        ));
    }
    if on.commits {
        parts.push((
            "Commits",
            section(
                "Commits",
                &commits::select_commits(&source.blocks, cap(0.05)),
            ),
        ));
    }
    if on.errors {
        parts.push((
            "Open Errors",
            section(
                "Open Errors",
                &history::select_open_errors(&source.blocks, cap(0.06)),
            ),
        ));
    }

    let head: Vec<&(&str, String)> = parts.iter().filter(|(_, text)| !text.is_empty()).collect();
    let head_text = head
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut sections: Vec<String> = head.iter().map(|(name, _)| (*name).to_owned()).collect();

    let mut transcript = String::new();
    if on.transcript {
        let remaining =
            budget as i64 - char_len(&head_text) as i64 - char_len(RECALL_NOTE) as i64 - 40;
        let max = remaining.max(cap(0.18) as i64) as usize;
        transcript = history::select_transcript(&source.blocks, max, &input.file_ops);
        if !transcript.is_empty() {
            sections.push("Recent Transcript".into());
        }
    }
    let transcript_part = if transcript.is_empty() {
        String::new()
    } else {
        format!("[Recent Transcript]\n{transcript}")
    };
    let body: Vec<&str> = [head_text.as_str(), transcript_part.as_str()]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();
    if body.is_empty() {
        return Summary::default();
    }
    Summary {
        text: format!(
            "{}\n\n---\n\n{RECALL_NOTE}",
            redact_secrets(&body.join("\n\n"))
        ),
        sections,
    }
}
