//! A lossless compaction plan for a branch: cut, calibrated estimates, summary, details.

use serde_json::{Value, json};

use crate::config::Config;
use crate::cut::{
    BudgetCut, TailBudget, apply_tail_budget, build_own_cut, calibrate_chars_per_token,
    collect_live, content_chars, resolve_smart_keep, tail_tokens_from, tokens_from_chars,
};
use crate::rank::FileOps;
use crate::source::collect_summary_source;
use crate::summary::{SummaryInput, auto_budget_tokens, build_lossless_summary};
use crate::text::{char_len, str_of};

pub const COMPACTOR_ID: &str = "@pi-unipi/compactor";
pub const SUMMARY_CHARS_PER_TOKEN: f64 = 4.0;
const TAIL_WINDOW_LIMIT: f64 = 0.4;
const TAIL_WINDOW_SHARE: f64 = 0.25;

#[derive(Debug, Clone, Default)]
pub struct PlanInput {
    pub branch: Vec<Value>,
    pub tokens_before: Option<u64>,
    pub previous_summary: Option<String>,
    pub file_ops: FileOps,
    pub keep: Option<usize>,
    pub keep_explicit: bool,
    pub context_window: Option<u64>,
    pub config: Config,
    pub cwd: Option<String>,
    pub reason: Option<String>,
    pub active_work: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    pub summarized: usize,
    pub kept: usize,
    pub tokens_before: Option<u64>,
    pub tokens_after_est: u64,
    pub kept_user_turns: usize,
    pub total_user_turns: usize,
    pub kept_tokens_est: u64,
    pub smart_keep_adjusted: bool,
    pub budget_cut: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub summary: String,
    pub first_kept_entry_id: String,
    pub details: Value,
    pub stats: Stats,
}

pub fn plan_lossless(input: &PlanInput) -> Result<Plan, &'static str> {
    let branch = &input.branch;
    let config = &input.config;
    let explicit = input.keep_explicit;
    let live_chars: usize = build_own_cut(branch, 0)
        .map(|_| {
            collect_live(branch)
                .iter()
                .map(|l| content_chars(&l.message["content"]))
                .sum()
        })
        .unwrap_or(0);
    let previous = input.previous_summary.as_deref().map_or(0, char_len);
    let cpt = calibrate_chars_per_token(live_chars + previous, input.tokens_before);

    let requested = explicit.then(|| input.keep.unwrap_or(1));
    let smart = resolve_smart_keep(branch, requested, explicit, config.smart_keep_tail, cpt);
    let mut cut = build_own_cut(branch, smart.keep)?;
    if !explicit {
        cut = apply_tail_budget(branch, cut, TailBudget::new(cpt));
    }
    if let Some(window) = input.context_window.filter(|w| *w > 0) {
        let budget = TailBudget {
            max_tokens: (window as f64 * TAIL_WINDOW_SHARE).floor() as u64,
            oversized_factor: TAIL_WINDOW_LIMIT / TAIL_WINDOW_SHARE,
            chars_per_token: cpt,
        };
        cut = apply_tail_budget(branch, cut, budget);
    }

    let kept_idx = (!cut.first_kept_entry_id.is_empty())
        .then(|| {
            branch
                .iter()
                .position(|e| str_of(e, "id") == cut.first_kept_entry_id)
        })
        .flatten();
    let end = kept_idx.unwrap_or(branch.len());
    let kept_tokens = kept_idx.map_or(0, |i| tail_tokens_from(branch, i, cpt));

    let source = collect_summary_source(branch, end);
    let wanted = if config.summary_budget_tokens > 0 {
        config.summary_budget_tokens
    } else {
        auto_budget_tokens(source.blocks.len())
    };
    let budget_tokens = match input.context_window.filter(|w| *w > 0) {
        Some(window) => wanted.min(((window as f64 * 0.08).floor() as usize).max(800)),
        None => wanted,
    };
    let summary = build_lossless_summary(&SummaryInput {
        source,
        active_work: if config.sections.active_work {
            input.active_work.clone()
        } else {
            Vec::new()
        },
        budget_chars: (budget_tokens as f64 * SUMMARY_CHARS_PER_TOKEN) as usize,
        cwd: input.cwd.clone(),
        file_ops: input.file_ops.clone(),
        sections: config.sections.clone(),
    });

    let tokens_after =
        kept_tokens + tokens_from_chars(char_len(&summary.text), SUMMARY_CHARS_PER_TOKEN);
    let kept = kept_idx.map_or(0, |i| {
        branch[i..]
            .iter()
            .filter(|e| str_of(e, "type") == "message")
            .count()
    });
    let stats = Stats {
        summarized: cut.summarized,
        kept,
        tokens_before: input.tokens_before,
        tokens_after_est: tokens_after,
        kept_user_turns: cut.kept_user_turns,
        total_user_turns: cut.total_user_turns,
        kept_tokens_est: kept_tokens,
        smart_keep_adjusted: smart.adjusted,
        budget_cut: cut.budget_cut.map(BudgetCut::as_str),
    };
    let details = json!({
        "compactor": COMPACTOR_ID,
        "version": 2,
        "method": "vcc",
        "sections": summary.sections,
        "sourceMessageCount": cut.summarized,
        "tokensAfter": tokens_after,
        "budgetTokens": budget_tokens,
        "reason": input.reason,
        "budgetCut": stats.budget_cut,
    });
    Ok(Plan {
        summary: summary.text,
        first_kept_entry_id: cut.first_kept_entry_id,
        details,
        stats,
    })
}

/// `formatTokens`: 1234 is `1.2k`, 1500000 is `1.5M`.
pub fn format_tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..10_000 => format!("{:.1}k", n as f64 / 1000.0),
        10_000..1_000_000 => format!("{}k", (n as f64 / 1000.0).round()),
        1_000_000..10_000_000 => format!("{:.1}M", n as f64 / 1_000_000.0),
        _ => format!("{}M", (n as f64 / 1_000_000.0).round()),
    }
}

/// The one-line notice for a lossless compaction.
pub fn format_stats(stats: &Stats) -> String {
    let before = stats
        .tokens_before
        .map(|b| {
            format!(
                "{} → ~{} tok, ",
                format_tokens(b),
                format_tokens(stats.tokens_after_est)
            )
        })
        .unwrap_or_default();
    if let Some(cut) = stats.budget_cut {
        let reason = if cut == "no_anchor" {
            "no user turn to anchor on"
        } else {
            "oversized tail"
        };
        return format!(
            "compactor: {before}kept ~{} tok tail ({reason}), summarized {}.",
            format_tokens(stats.kept_tokens_est),
            stats.summarized
        );
    }
    let mut notes = vec![format!("summarized {}", stats.summarized)];
    if stats.smart_keep_adjusted {
        notes.push("smart-keep".into());
    }
    format!(
        "compactor: {before}kept {}/{} turns ({}).",
        stats.kept_user_turns,
        stats.total_user_turns,
        notes.join(", ")
    )
}
