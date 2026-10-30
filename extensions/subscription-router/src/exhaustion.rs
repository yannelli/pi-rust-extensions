//! Confirmed quota blocks per (provider, family). A block lasts until the reported reset, or
//! backs off 5 minutes doubling to 60 when the reset is unknown.

use std::collections::HashMap;

use serde::Serialize;

use crate::config::ModelRef;
use crate::usage::{Scope, UsageDecision, model_matches_family};

const BACKOFF_BASE_MS: i64 = 5 * 60_000;
const BACKOFF_CAP_MS: i64 = 60 * 60_000;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExhaustionMark {
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub families: Option<Vec<String>>,
    pub window: String,
    pub until: i64,
    pub reset_known: bool,
    pub confirmations: u32,
}

impl ExhaustionMark {
    pub fn covers(&self, model: &ModelRef) -> bool {
        self.provider == model.provider
            && self.families.as_ref().is_none_or(|f| {
                f.iter()
                    .any(|family| model_matches_family(&model.id, family))
            })
    }
}

#[derive(Default)]
pub struct ExhaustionTracker {
    marks: HashMap<String, ExhaustionMark>,
}

fn scope_key(provider: &str, families: Option<&Vec<String>>) -> String {
    match families.filter(|f| !f.is_empty()) {
        Some(families) => {
            let mut sorted = families.clone();
            sorted.sort();
            format!("{provider}:{}", sorted.join(","))
        }
        None => format!("{provider}:*"),
    }
}

impl ExhaustionTracker {
    /// Records a confirmed block for the scope of `decision`.
    pub fn mark(&mut self, model: &ModelRef, decision: &UsageDecision, now: i64) -> ExhaustionMark {
        let families = match decision.scope {
            Some(Scope::Model) => decision.families.clone(),
            _ => None,
        };
        let key = scope_key(&model.provider, families.as_ref());
        let previous = self.marks.get(&key);
        let recent = previous.is_some_and(|p| now - p.until < BACKOFF_CAP_MS);
        let confirmations = if recent {
            previous.map_or(0, |p| p.confirmations)
        } else {
            0
        } + 1;
        let reset = decision.reset_at.filter(|r| *r > now);
        let shift = confirmations.saturating_sub(1).min(10);
        let backoff = (BACKOFF_BASE_MS << shift).min(BACKOFF_CAP_MS);
        let mark = ExhaustionMark {
            provider: model.provider.clone(),
            families,
            window: decision
                .window
                .clone()
                .unwrap_or_else(|| "included quota".into()),
            until: reset.unwrap_or(now + backoff),
            reset_known: reset.is_some(),
            confirmations,
        };
        self.marks.insert(key, mark.clone());
        mark
    }

    /// The live mark covering `model`. Expired marks stay so a quick re-confirmation grows the backoff.
    pub fn blocking(&self, model: &ModelRef, now: i64) -> Option<&ExhaustionMark> {
        self.marks
            .values()
            .find(|m| m.until > now && m.covers(model))
    }

    pub fn active(&self, now: i64) -> Vec<ExhaustionMark> {
        let mut marks: Vec<_> = self
            .marks
            .values()
            .filter(|m| m.until > now)
            .cloned()
            .collect();
        marks.sort_by(|a, b| a.provider.cmp(&b.provider).then(a.until.cmp(&b.until)));
        marks
    }

    pub fn clear(&mut self) {
        self.marks.clear();
    }

    pub fn clear_provider(&mut self, provider: &str) {
        self.marks.retain(|_, m| m.provider != provider);
    }
}
