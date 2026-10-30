//! Compaction stats from the branch, the compaction card, doctor checks, and the budget text.

use serde_json::{Map, Value, json};

use crate::config::Method;
use crate::cut::content_chars;
use crate::plan::{COMPACTOR_ID, SUMMARY_CHARS_PER_TOKEN, Stats, format_tokens};
use crate::text::{char_len, str_of};

pub const CARD_TYPE: &str = "unipi-compaction";

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub method: Method,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionStats {
    pub compactions: Vec<Record>,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub tokens_saved: u64,
}

pub fn method_of(details: &Value) -> Method {
    if str_of(details, "compactor") == COMPACTOR_ID && str_of(details, "method") != "llm" {
        Method::Vcc
    } else {
        Method::Llm
    }
}

pub fn session_stats(branch: &[Value]) -> SessionStats {
    let mut compactions = Vec::new();
    for (i, entry) in branch
        .iter()
        .enumerate()
        .filter(|(_, e)| str_of(e, "type") == "compaction")
    {
        let before = entry["tokensBefore"].as_u64().unwrap_or(0);
        let details = &entry["details"];
        let mut after = details["tokensAfter"].as_u64().unwrap_or(0);
        if after == 0 {
            let kept_id = str_of(entry, "firstKeptEntryId");
            let kept = (!kept_id.is_empty())
                .then(|| branch.iter().position(|e| str_of(e, "id") == kept_id))
                .flatten();
            let mut chars = char_len(str_of(entry, "summary"));
            if let Some(k) = kept.filter(|k| *k < i) {
                chars += branch[k..i]
                    .iter()
                    .filter(|e| str_of(e, "type") == "message")
                    .map(|e| content_chars(&e["message"]["content"]))
                    .sum::<usize>();
            }
            after = (chars as f64 / SUMMARY_CHARS_PER_TOKEN).ceil() as u64;
        }
        compactions.push(Record {
            tokens_before: before,
            tokens_after: after,
            method: method_of(details),
        });
    }
    let tokens_before = compactions.iter().map(|c| c.tokens_before).sum();
    let tokens_after = compactions
        .iter()
        .map(|c| {
            c.tokens_after.min(if c.tokens_before > 0 {
                c.tokens_before
            } else {
                c.tokens_after
            })
        })
        .sum();
    SessionStats {
        compactions,
        tokens_before,
        tokens_after,
        tokens_saved: u64::saturating_sub(tokens_before, tokens_after),
    }
}

pub fn method_breakdown(stats: &SessionStats) -> String {
    let count = |m: Method| stats.compactions.iter().filter(|c| c.method == m).count();
    let mut parts = Vec::new();
    for (method, word) in [(Method::Vcc, "lossless"), (Method::Llm, "model")] {
        let n = count(method);
        if n > 0 {
            parts.push((
                stats.compactions.iter().position(|c| c.method == method),
                format!("{n} {word}"),
            ));
        }
    }
    parts.sort_by_key(|(at, _)| *at);
    parts
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn stats_text(branch: &[Value], usage: Option<&Value>) -> String {
    let stats = session_stats(branch);
    let mut lines = vec!["Compactor — this session".to_owned()];
    let count = stats.compactions.len();
    let breakdown = if count > 0 {
        format!(" ({})", method_breakdown(&stats))
    } else {
        String::new()
    };
    lines.push(format!("Compactions: {count}{breakdown}"));
    lines.push(if stats.tokens_before > 0 {
        format!(
            "Tokens: {} → {} (−{})",
            format_tokens(stats.tokens_before),
            format_tokens(stats.tokens_after),
            format_tokens(stats.tokens_saved)
        )
    } else {
        "Tokens: nothing compacted yet".into()
    });
    if let Some(usage) = usage
        && let (Some(tokens), Some(window)) = (
            usage["tokens"].as_u64(),
            usage["contextWindow"].as_u64().filter(|w| *w > 0),
        )
    {
        lines.push(format!(
            "Context now: ~{} / {}",
            format_tokens(tokens),
            format_tokens(window)
        ));
    }
    lines.join("\n")
}

/// Card data for the `unipi-compaction` entry.
pub struct Card<'a> {
    pub method: Method,
    pub trigger: &'a str,
    pub command: Option<&'a str>,
    pub tokens_before: u64,
    pub stats: Option<&'a Stats>,
    pub details: Option<&'a Value>,
    pub summary: Option<&'a str>,
    pub percent: Option<f64>,
    pub threshold: Option<f64>,
}

impl Card<'_> {
    pub fn data(&self) -> Value {
        let mut data = Map::new();
        data.insert("method".into(), json!(self.method.as_str()));
        data.insert("trigger".into(), json!(self.trigger));
        if let Some(command) = self.command {
            data.insert("command".into(), json!(command));
        }
        data.insert("tokensBefore".into(), json!(self.tokens_before));
        if let Some(stats) = self.stats {
            data.insert("tokensAfter".into(), json!(stats.tokens_after_est));
            data.insert("keptTurns".into(), json!(stats.kept_user_turns));
            data.insert("totalTurns".into(), json!(stats.total_user_turns));
            data.insert("keptTokens".into(), json!(stats.kept_tokens_est));
        }
        let summary_chars = self.summary.map_or(0, char_len);
        if summary_chars > 0 {
            data.insert(
                "summaryTokens".into(),
                json!((summary_chars as f64 / SUMMARY_CHARS_PER_TOKEN).ceil() as u64),
            );
        }
        if let Some(sections) = self
            .details
            .and_then(|d| d.get("sections"))
            .filter(|s| s.is_array())
        {
            data.insert("sections".into(), sections.clone());
        }
        if let Some(percent) = self.percent {
            data.insert("percent".into(), json!(percent));
        }
        if let Some(threshold) = self.threshold {
            data.insert("threshold".into(), json!(threshold));
        }
        Value::Object(data)
    }
}
