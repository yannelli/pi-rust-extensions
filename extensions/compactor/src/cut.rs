//! Where a compaction cuts: keep:N user-turn tails, token-budget rescue, smart keep.

use serde_json::{Value, json};

use crate::text::str_of;
pub use crate::tokens::{
    DEFAULT_CHARS_PER_TOKEN, IMAGE_CONTENT_CHARS, calibrate_chars_per_token, content_chars,
    tokens_from_chars,
};

pub const OVERSIZED_TAIL_FACTOR: f64 = 2.5;
pub const MIN_SMART_TAIL_TOKENS: u64 = 5_000;
pub const MAX_SMART_TAIL_TOKENS: u64 = 25_000;

fn message_tokens(message: &Value, cpt: f64) -> u64 {
    tokens_from_chars(content_chars(&message["content"]), cpt)
}

/// A message in the live window and the branch entry it came from.
#[derive(Debug, Clone)]
pub struct Live {
    pub entry: usize,
    pub id: String,
    pub message: Value,
}

fn live_message(entry: &Value) -> Option<Value> {
    match str_of(entry, "type") {
        "message" => entry.get("message").cloned(),
        "custom_message" => Some(
            json!({ "role": "custom", "customType": entry["customType"], "content": entry["content"] }),
        ),
        "branch_summary" => Some(json!({ "role": "branchSummary", "summary": entry["summary"] })),
        _ => None,
    }
}

/// Messages after the last compaction's kept entry, with orphan recovery for `""` or missing ids.
pub fn collect_live(branch: &[Value]) -> Vec<Live> {
    let last = branch
        .iter()
        .rposition(|e| str_of(e, "type") == "compaction");
    let kept_id = last
        .map(|i| str_of(&branch[i], "firstKeptEntryId"))
        .unwrap_or_default();
    let valid_kept = !kept_id.is_empty() && branch.iter().any(|e| str_of(e, "id") == kept_id);
    let start = match last {
        Some(i) if !valid_kept => i + 1,
        Some(_) => branch
            .iter()
            .position(|e| str_of(e, "id") == kept_id)
            .unwrap_or(0),
        None => 0,
    };
    branch
        .iter()
        .enumerate()
        .skip(start)
        .filter(|(_, e)| str_of(e, "type") != "compaction")
        .filter_map(|(i, e)| {
            let message = live_message(e)?;
            Some(Live {
                entry: i,
                id: str_of(e, "id").to_owned(),
                message,
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetCut {
    NoAnchor,
    OversizedTail,
}

impl BudgetCut {
    pub fn as_str(self) -> &'static str {
        match self {
            BudgetCut::NoAnchor => "no_anchor",
            BudgetCut::OversizedTail => "oversized_tail",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Cut {
    /// Number of live messages summarized (the cut index into the live window).
    pub summarized: usize,
    pub first_kept_entry_id: String,
    pub compact_all: bool,
    pub kept_user_turns: usize,
    pub total_user_turns: usize,
    pub requested_keep: usize,
    pub keep_fallback_to_compact_all: bool,
    pub budget_cut: Option<BudgetCut>,
}

pub type CutResult = Result<Cut, &'static str>;

fn is_user(live: &Live) -> bool {
    str_of(&live.message, "role") == "user"
}

pub fn build_own_cut(branch: &[Value], keep: usize) -> CutResult {
    let live = collect_live(branch);
    if live.is_empty() {
        return Err("no_live_messages");
    }
    if live.len() <= 2 {
        return Err("too_few_live_messages");
    }
    let users: Vec<usize> = (0..live.len()).filter(|i| is_user(&live[*i])).collect();
    let compact_all = |fallback: bool| Cut {
        summarized: live.len(),
        first_kept_entry_id: String::new(),
        compact_all: true,
        kept_user_turns: 0,
        total_user_turns: users.len(),
        requested_keep: keep,
        keep_fallback_to_compact_all: fallback,
        budget_cut: None,
    };
    if keep == 0 {
        return Ok(compact_all(false));
    }
    let cut_idx = users.len().checked_sub(keep).map(|target| users[target]);
    match cut_idx {
        Some(idx) if idx > 0 => Ok(Cut {
            summarized: idx,
            first_kept_entry_id: live[idx].id.clone(),
            compact_all: false,
            kept_user_turns: keep,
            total_user_turns: users.len(),
            requested_keep: keep,
            keep_fallback_to_compact_all: false,
            budget_cut: None,
        }),
        _ => Ok(compact_all(true)),
    }
}

/// First live index whose tail reaches `max_tokens`, moved forward off tool results.
pub fn find_budget_cut(live: &[Live], max_tokens: u64, cpt: f64) -> Option<usize> {
    let mut acc = 0;
    let crossed = (0..live.len()).rev().find(|i| {
        acc += message_tokens(&live[*i].message, cpt);
        acc >= max_tokens
    })?;
    (crossed.max(1)..live.len()).find(|j| str_of(&live[*j].message, "role") != "toolResult")
}

#[derive(Debug, Clone, Copy)]
pub struct TailBudget {
    pub max_tokens: u64,
    pub oversized_factor: f64,
    pub chars_per_token: f64,
}

impl TailBudget {
    pub fn new(chars_per_token: f64) -> Self {
        Self {
            max_tokens: MAX_SMART_TAIL_TOKENS,
            oversized_factor: OVERSIZED_TAIL_FACTOR,
            chars_per_token,
        }
    }
}

pub fn apply_tail_budget(branch: &[Value], cut: Cut, budget: TailBudget) -> Cut {
    let live = collect_live(branch);
    let users_from = |idx: usize| live[idx..].iter().filter(|l| is_user(l)).count();
    let budget_result = |idx: usize, kind: BudgetCut, cut: &Cut| Cut {
        summarized: idx,
        first_kept_entry_id: live[idx].id.clone(),
        compact_all: false,
        kept_user_turns: users_from(idx),
        total_user_turns: users_from(0),
        requested_keep: cut.requested_keep,
        keep_fallback_to_compact_all: false,
        budget_cut: Some(kind),
    };
    let cpt = budget.chars_per_token;
    if cut.compact_all {
        if !cut.keep_fallback_to_compact_all {
            return cut;
        }
        return match find_budget_cut(&live, budget.max_tokens, cpt) {
            Some(idx) => budget_result(idx, BudgetCut::NoAnchor, &cut),
            None => cut,
        };
    }
    let tail: u64 = live[cut.summarized.min(live.len())..]
        .iter()
        .map(|l| message_tokens(&l.message, cpt))
        .sum();
    if tail as f64 <= budget.max_tokens as f64 * budget.oversized_factor {
        return cut;
    }
    match find_budget_cut(&live, budget.max_tokens, cpt) {
        Some(idx) if idx > cut.summarized => budget_result(idx, BudgetCut::OversizedTail, &cut),
        _ => cut,
    }
}

/// Estimated tokens of the branch from entry `from` on (message entries only).
pub fn tail_tokens_from(branch: &[Value], from: usize, cpt: f64) -> u64 {
    let chars = branch[from..]
        .iter()
        .filter(|e| str_of(e, "type") == "message")
        .map(|e| content_chars(&e["message"]["content"]))
        .sum();
    tokens_from_chars(chars, cpt)
}

fn tail_tokens_for_keep(branch: &[Value], keep: usize, cpt: f64) -> Option<u64> {
    let cut = build_own_cut(branch, keep)
        .ok()
        .filter(|c| !c.compact_all)?;
    let idx = branch
        .iter()
        .position(|e| str_of(e, "id") == cut.first_kept_entry_id)?;
    Some(tail_tokens_from(branch, idx, cpt))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SmartKeep {
    pub keep: usize,
    pub adjusted: bool,
    pub from_keep: usize,
}

/// Grows the default keep:1 while the tail stays small (5k to 25k tokens).
pub fn resolve_smart_keep(
    branch: &[Value],
    requested: Option<usize>,
    explicit: bool,
    smart: bool,
    cpt: f64,
) -> SmartKeep {
    let base = requested.unwrap_or(1);
    let unchanged = SmartKeep {
        keep: base,
        adjusted: false,
        from_keep: base,
    };
    if explicit || !smart {
        return unchanged;
    }
    match tail_tokens_for_keep(branch, base, cpt) {
        Some(tokens) if tokens <= MIN_SMART_TAIL_TOKENS => {}
        _ => return unchanged,
    }
    let total = build_own_cut(branch, base)
        .map(|c| c.total_user_turns)
        .unwrap_or(0);
    let mut selected = base;
    for k in base + 1..=total {
        match tail_tokens_for_keep(branch, k, cpt) {
            Some(tokens) if tokens <= MAX_SMART_TAIL_TOKENS => selected = k,
            _ => break,
        }
    }
    SmartKeep {
        keep: selected,
        adjusted: selected != base,
        from_keep: base,
    }
}
