//! Admission for `router:rpc:admit` and the decision record of a scored pick.

use super::*;
use crate::config::{Config, ROUTER_PROVIDER};

impl Router {
    /// A physical model is refused only while blocked. A profile is refused only when some entry
    /// is quota-blocked and none is eligible.
    pub fn admit(&self, model: &str, config: &Config, env: &dyn RouteEnv) -> Admission {
        let Some(model) = ModelRef::parse(model) else {
            return Admission {
                ok: true,
                ..Default::default()
            };
        };
        let now = now_ms();
        if model.provider == ROUTER_PROVIDER {
            let Some(profile) = config.profile(&model.id) else {
                return Admission {
                    ok: true,
                    ..Default::default()
                };
            };
            return match self.pick(profile, env, None) {
                Pick::Chosen { entry, .. } => Admission {
                    ok: true,
                    resolved_model: Some(entry.model.key()),
                    ..Default::default()
                },
                Pick::Exhausted { blocked, .. } if !blocked.iter().any(|b| b.quota) => Admission {
                    ok: true,
                    ..Default::default()
                },
                Pick::Exhausted { reset_at, .. } => Admission {
                    ok: false,
                    resolved_model: None,
                    reset_at,
                    wait: Some(can_wait(profile.exhaustion.as_ref(), reset_at, now)),
                },
            };
        }
        match self.blocked_by(&model) {
            None => Admission {
                ok: true,
                resolved_model: Some(model.key()),
                ..Default::default()
            },
            Some((_, reset_at)) => Admission {
                ok: false,
                resolved_model: None,
                reset_at,
                wait: Some(can_wait(None, reset_at, now)),
            },
        }
    }
}

pub(super) fn decision(
    profile: &Profile,
    reason: Reason,
    entry: &ChainEntry,
    candidates: &[Candidate],
    blocked: &[Ineligible],
) -> RouteDecision {
    RouteDecision {
        at: now_ms(),
        profile: profile.id.clone(),
        strategy: profile.strategy().as_str().into(),
        reason,
        chosen: entry.model.key(),
        candidates: candidates
            .iter()
            .map(|c| DecisionCandidate {
                model: c.entry.model.key(),
                quota: c.score.clone(),
                age_ms: c.age_ms,
            })
            .collect(),
        blocked: blocked
            .iter()
            .map(|b| DecisionBlocked {
                model: b.entry.model.key(),
                reason: b.reason.clone(),
            })
            .collect(),
    }
}
