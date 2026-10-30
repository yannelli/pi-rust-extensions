//! Reset-aware ranking of chain candidates (upstream `selection.ts`).

use serde::Serialize;

use crate::config::{ModelRef, Strategy};
use crate::usage::{Status, UsageSnapshot, applies_to};

/// A snapshot past its TTL still counts as measured for this long (three TTLs).
pub const STALE_GRACE_MS: i64 = 15 * 60_000;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum QuotaScore {
    Unlimited,
    Measured {
        score: f64,
        window: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        roundoff: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stale: Option<i64>,
    },
    Fallback {
        reason: String,
    },
}

fn fallback(reason: &str) -> QuotaScore {
    QuotaScore::Fallback {
        reason: reason.into(),
    }
}

/// The tightest `remaining / clamp(timeLeft, 0.05, 1)` over the windows that apply to `model`.
pub fn score_quota(model: &ModelRef, snapshot: Option<&UsageSnapshot>, now: i64) -> QuotaScore {
    let Some(snapshot) = snapshot else {
        return fallback("usage missing");
    };
    if !matches!(snapshot.status, Status::Available | Status::Stale) {
        let status = serde_json::to_value(snapshot.status).unwrap_or_default();
        return fallback(&format!("usage {}", status.as_str().unwrap_or("error")));
    }
    let age = (now - snapshot.retrieved_at).max(0);
    let expired = snapshot.status == Status::Stale || snapshot.expires_at <= now;
    if expired && age > STALE_GRACE_MS {
        return fallback("usage stale");
    }
    if !expired && snapshot.credits.as_ref().is_some_and(|c| c.unlimited) {
        return QuotaScore::Unlimited;
    }
    let mut limiting: Option<(f64, String, f64)> = None;
    for window in &snapshot.windows {
        if !applies_to(window, &model.id) || window.reset_at.is_some_and(|r| r <= now) {
            continue;
        }
        if !window.used_percent.is_finite() || window.used_percent < 0.0 {
            continue;
        }
        let time_left = match (window.reset_at, window.duration_ms) {
            (Some(reset), Some(duration)) if duration > 0 => {
                ((reset - now) as f64 / duration as f64).clamp(0.05, 1.0)
            }
            _ => 1.0,
        };
        let score = (1.0 - window.used_percent.min(100.0) / 100.0) / time_left;
        if limiting.as_ref().is_none_or(|(best, _, _)| score < *best) {
            limiting = Some((score, window.name.clone(), f64::EPSILON * 4.0 / time_left));
        }
    }
    let Some((score, window, roundoff)) = limiting else {
        return fallback("no applicable active windows");
    };
    let stale = expired.then_some(age);
    QuotaScore::Measured {
        score,
        window,
        roundoff: Some(roundoff),
        stale,
    }
}

fn better(a: &QuotaScore, b: &QuotaScore) -> bool {
    match (a, b) {
        (QuotaScore::Unlimited, b) => !matches!(b, QuotaScore::Unlimited),
        (_, QuotaScore::Unlimited) | (QuotaScore::Fallback { .. }, _) => false,
        (_, QuotaScore::Fallback { .. }) => true,
        (QuotaScore::Measured { score: a, .. }, QuotaScore::Measured { score: b, .. }) => a > b,
    }
}

/// A challenger must beat the incumbent by more than 20%, with float roundoff kept sticky.
fn beats_hysteresis(challenger: (f64, f64), incumbent: (f64, f64)) -> bool {
    if incumbent.0 == 0.0 {
        return challenger.0 > 0.0;
    }
    let threshold = incumbent.0 * 1.2;
    let roundoff = (challenger.0.abs() + threshold.abs()) * f64::EPSILON * 4.0
        + challenger.1
        + incumbent.1 * 1.2;
    challenger.0 - threshold > roundoff
}

fn measured(score: &QuotaScore) -> Option<(f64, f64)> {
    match score {
        QuotaScore::Measured {
            score, roundoff, ..
        } => Some((*score, roundoff.unwrap_or(0.0))),
        _ => None,
    }
}

/// The index of the selected candidate, or `None` without candidates.
pub fn select_candidate(
    scores: &[QuotaScore],
    strategy: Strategy,
    incumbent: Option<usize>,
) -> Option<usize> {
    let first = scores.first()?;
    if strategy == Strategy::Ordered {
        return Some(0);
    }
    let (mut best, mut challenger) = (0, first);
    for (i, score) in scores.iter().enumerate() {
        if better(score, challenger) {
            best = i;
            challenger = score;
        }
    }
    if let Some((index, current)) = incumbent.and_then(|i| Some((i, scores.get(i)?))) {
        match (current, challenger) {
            (QuotaScore::Unlimited, _) => return Some(index),
            (QuotaScore::Fallback { .. }, QuotaScore::Fallback { .. }) => return Some(index),
            _ => {}
        }
        if let (Some(current), Some(challenger)) = (measured(current), measured(challenger)) {
            if !beats_hysteresis(challenger, current) {
                return Some(index);
            }
        }
    }
    Some(best)
}

pub fn format_score(score: &QuotaScore) -> String {
    match score {
        QuotaScore::Fallback { reason } => format!("fallback: {reason}"),
        QuotaScore::Unlimited => "unlimited".into(),
        QuotaScore::Measured {
            score,
            window,
            stale,
            ..
        } => {
            let stale = stale.map_or(String::new(), |ms| format!(", stale {}m", ms / 60_000));
            format!("score {score:.3}, limiting {window}{stale}")
        }
    }
}
