//! Chain routing for `router/<profile>`. Eligibility reads only cached data; the one network
//! read is `confirm`, a usage read after a quota-looking failure, and only it creates blocks.

mod admit;
use admit::decision;
mod types;

pub use types::*;

use std::sync::{Arc, Mutex, MutexGuard};

use crate::config::{ChainEntry, ModelRef, Profile, Strategy};
use crate::exhaustion::ExhaustionTracker;
use crate::fsutil::{iso_seconds, now_ms};
use crate::selection::{QuotaScore, score_quota, select_candidate};
use crate::usage::{BoxFuture, UsageDecision, UsageService, decision_for};

/// What routing needs from pi: which chain models exist with credentials, and a fresh usage read.
pub trait RouteEnv: Send + Sync {
    fn available(&self, model: &ModelRef) -> bool;
    /// The decision from a usage read that started at or after `not_before`; `None` when unread.
    fn refresh<'a>(
        &'a self,
        model: &'a ModelRef,
        not_before: i64,
    ) -> BoxFuture<'a, Option<UsageDecision>>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub entry: ChainEntry,
    pub score: QuotaScore,
    pub age_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    Chosen {
        entry: ChainEntry,
        candidates: Vec<Candidate>,
        blocked: Vec<Ineligible>,
    },
    Exhausted {
        blocked: Vec<Ineligible>,
        reset_at: Option<i64>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Routed {
    pub entry: ChainEntry,
    pub thinking_level: String,
    /// Set for scored picks; continuations and same-model retries have none.
    pub decision: Option<RouteDecision>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RouteError {
    Exhausted {
        message: String,
        reset_at: Option<i64>,
    },
    Partial,
}

impl RouteError {
    pub fn message(&self) -> String {
        match self {
            Self::Exhausted { message, .. } => message.clone(),
            Self::Partial => {
                "subscription-router: refusing to replay a partial response; retry explicitly."
                    .into()
            }
        }
    }
}

pub struct Router {
    pub usage: Arc<UsageService>,
    tracker: Mutex<ExhaustionTracker>,
}

impl Router {
    pub fn new(usage: Arc<UsageService>) -> Self {
        Self {
            usage,
            tracker: Mutex::new(ExhaustionTracker::default()),
        }
    }

    pub fn tracker(&self) -> MutexGuard<'_, ExhaustionTracker> {
        self.tracker.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Why `model` is blocked: a confirmed mark, or cached usage that shows its window exhausted.
    pub fn blocked_by(&self, model: &ModelRef) -> Option<(String, Option<i64>)> {
        let now = now_ms();
        if let Some(mark) = self.tracker().blocking(model, now) {
            let kind = if mark.reset_known {
                "resets"
            } else {
                "backing off until"
            };
            return Some((
                format!(
                    "{} exhausted, {kind} {}",
                    mark.window,
                    iso_seconds(mark.until)
                ),
                Some(mark.until),
            ));
        }
        let decision = decision_for(model, self.usage.peek(&model.provider).as_ref(), now);
        decision.block.then(|| {
            let window = decision.window.unwrap_or_else(|| "quota".into());
            (format!("{window} exhausted"), decision.reset_at)
        })
    }

    pub fn pick(
        &self,
        profile: &Profile,
        env: &dyn RouteEnv,
        incumbent: Option<&ModelRef>,
    ) -> Pick {
        let now = now_ms();
        let (mut candidates, mut blocked) = (Vec::new(), Vec::new());
        for entry in &profile.chain {
            if !env.available(&entry.model) {
                let reason = "not available (unknown model or no credentials)".into();
                blocked.push(Ineligible {
                    entry: entry.clone(),
                    reason,
                    reset_at: None,
                    quota: false,
                });
                continue;
            }
            if let Some((reason, reset_at)) = self.blocked_by(&entry.model) {
                blocked.push(Ineligible {
                    entry: entry.clone(),
                    reason,
                    reset_at,
                    quota: true,
                });
                continue;
            }
            let snapshot = self.usage.peek(&entry.model.provider);
            candidates.push(Candidate {
                entry: entry.clone(),
                score: score_quota(&entry.model, snapshot.as_ref(), now),
                age_ms: snapshot.map(|s| (now - s.retrieved_at).max(0)),
            });
        }
        let scores: Vec<QuotaScore> = candidates.iter().map(|c| c.score.clone()).collect();
        let incumbent = incumbent.and_then(|m| candidates.iter().position(|c| &c.entry.model == m));
        match select_candidate(&scores, profile.strategy(), incumbent) {
            Some(index) => Pick::Chosen {
                entry: candidates[index].entry.clone(),
                candidates,
                blocked,
            },
            None => {
                let reset_at = blocked.iter().filter_map(|b| b.reset_at).min();
                Pick::Exhausted { blocked, reset_at }
            }
        }
    }

    /// Marks and returns true only when a usage read after `failed_at` shows `model` blocked.
    pub async fn confirm(&self, model: &ModelRef, env: &dyn RouteEnv, failed_at: i64) -> bool {
        let Some(decision) = env.refresh(model, failed_at).await.filter(|d| d.block) else {
            return false;
        };
        self.tracker().mark(model, &decision, now_ms());
        true
    }

    /// `retry` after a confirmed quota error reselects; any other failure retries the same model.
    /// `continuation` (and balanced `direct`) keep an eligible previous model; the rest select.
    pub async fn route(
        &self,
        profile: &Profile,
        input: &RouteInput,
        env: &dyn RouteEnv,
    ) -> Result<Routed, RouteError> {
        let routed = |entry: &ChainEntry, decision| Routed {
            entry: entry.clone(),
            thinking_level: entry
                .thinking
                .clone()
                .unwrap_or_else(|| input.thinking_level.clone()),
            decision,
        };
        if let (Reason::Retry, Some(failed)) = (input.reason, &input.failed) {
            if let Some(entry) = profile.entry(&failed.model) {
                let confirmed =
                    failed.quota && self.confirm(&failed.model, env, failed.timestamp).await;
                if !confirmed {
                    return Ok(routed(entry, None));
                }
                if failed.partial {
                    return Err(RouteError::Partial);
                }
            }
        }
        let sticky = input.reason == Reason::Continuation
            || (input.reason == Reason::Direct && profile.strategy() == Strategy::Balanced);
        if let (true, Some(previous)) = (sticky, &input.previous) {
            if let Some(entry) = profile.entry(previous) {
                if self.blocked_by(previous).is_none() && env.available(previous) {
                    return Ok(routed(entry, None));
                }
            }
        }
        let balanced_user =
            input.reason == Reason::User && profile.strategy() == Strategy::Balanced;
        let incumbent = input.previous.as_ref().filter(|_| balanced_user);
        match self.pick(profile, env, incumbent) {
            Pick::Chosen {
                entry,
                candidates,
                blocked,
            } => {
                let decision = decision(profile, input.reason, &entry, &candidates, &blocked);
                Ok(routed(&entry, Some(decision)))
            }
            Pick::Exhausted { blocked, reset_at } => {
                let message = exhausted_message(&profile.id, &blocked, reset_at);
                Err(RouteError::Exhausted { message, reset_at })
            }
        }
    }
}
