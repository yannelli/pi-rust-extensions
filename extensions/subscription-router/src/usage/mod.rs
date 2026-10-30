//! Normalized subscription quota: snapshots per provider, block decisions, and text forms.

mod fetch;
mod parse;
mod service;

pub use fetch::retry_after_ms;
pub use parse::{codex_account_id, parse_anthropic, parse_codex};
pub use service::{BoxFuture, GetOptions, Tokens, Urls, UsageService};

use serde::{Deserialize, Serialize};

use crate::config::ModelRef;
use crate::fsutil::{iso_minute, iso_seconds};

pub const CACHE_TTL_MS: i64 = 5 * 60_000;
pub const FIVE_HOURS_MS: i64 = 18_000_000;
pub const SEVEN_DAYS_MS: i64 = 604_800_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Available,
    Stale,
    Unavailable,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Provider,
    Model,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub name: String,
    pub used_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_source: Option<String>,
    pub scope: Scope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub families: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Credits {
    pub available: bool,
    pub unlimited: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub provider: String,
    pub status: Status,
    pub windows: Vec<UsageWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits: Option<Credits>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    pub retrieved_at: i64,
    pub expires_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl UsageSnapshot {
    pub fn error(provider: &str, status: Status, now: i64, message: impl Into<String>) -> Self {
        Self {
            provider: provider.to_owned(),
            status,
            windows: Vec::new(),
            credits: None,
            tier: None,
            retrieved_at: now,
            expires_at: now,
            message: Some(message.into()),
        }
    }

    pub fn stale(&self, now: i64, message: Option<String>) -> Self {
        Self {
            status: Status::Stale,
            expires_at: now,
            message,
            ..self.clone()
        }
    }

    pub fn measured(&self) -> bool {
        matches!(self.status, Status::Available | Status::Stale) && !self.windows.is_empty()
    }

    pub fn fresh(&self, now: i64) -> bool {
        self.status == Status::Available && self.expires_at > now
    }

    pub fn window_of(&self, duration: i64) -> Option<&UsageWindow> {
        self.windows
            .iter()
            .find(|w| w.scope == Scope::Provider && w.duration_ms == Some(duration))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDecision {
    pub block: bool,
    pub message: Option<String>,
    pub window: Option<String>,
    pub reset_at: Option<i64>,
    pub scope: Option<Scope>,
    pub families: Option<Vec<String>>,
}

/// `family` appears in `model_id` as a whole token bounded by an edge or one of `-./:`.
pub fn model_matches_family(model_id: &str, family: &str) -> bool {
    let (id, family) = (model_id.to_lowercase(), family.to_lowercase());
    let bounded = |c: Option<char>| c.is_none_or(|c| "-./:".contains(c));
    id.match_indices(&family).any(|(start, _)| {
        let before = id[..start].chars().next_back();
        let after = id[start + family.len()..].chars().next();
        bounded(before) && bounded(after)
    })
}

pub fn applies_to(window: &UsageWindow, model_id: &str) -> bool {
    window.scope == Scope::Provider
        || window.families.as_ref().is_some_and(|f| {
            f.iter()
                .any(|family| model_matches_family(model_id, family))
        })
}

/// Blocks on an exhausted matching window in fresh data, or in stale data whose reset is ahead.
pub fn decision_for(model: &ModelRef, snapshot: Option<&UsageSnapshot>, now: i64) -> UsageDecision {
    let Some(snapshot) = snapshot.filter(|s| matches!(s.status, Status::Available | Status::Stale))
    else {
        return UsageDecision::default();
    };
    if snapshot.credits.as_ref().is_some_and(|c| c.unlimited) {
        return UsageDecision::default();
    }
    let exhausted = snapshot.windows.iter().find(|w| {
        w.used_percent >= 100.0
            && applies_to(w, &model.id)
            && w.reset_at
                .map_or(snapshot.status == Status::Available, |r| r > now)
    });
    let Some(window) = exhausted else {
        return UsageDecision::default();
    };
    let reset = window
        .reset_at
        .map_or("an unknown time".into(), iso_seconds);
    UsageDecision {
        block: true,
        message: Some(format!(
            "{}: {} is exhausted and resets {reset}.",
            model.key(),
            window.name
        )),
        window: Some(window.name.clone()),
        reset_at: window.reset_at,
        scope: Some(window.scope),
        families: window.families.clone(),
    }
}

pub fn format(snapshot: Option<&UsageSnapshot>) -> String {
    let Some(snapshot) = snapshot else {
        return "usage unavailable (not checked)".into();
    };
    if snapshot.status != Status::Available {
        let message = snapshot.message.as_deref().unwrap_or("not available");
        let status = serde_json::to_value(snapshot.status).unwrap_or_default();
        return format!("usage {}: {message}", status.as_str().unwrap_or("error"));
    }
    let windows: Vec<String> = snapshot
        .windows
        .iter()
        .map(|w| {
            let reset = w
                .reset_at
                .map_or(String::new(), |r| format!(", resets {}", iso_seconds(r)));
            format!("{} {}% used{reset}", w.name, w.used_percent.round())
        })
        .collect();
    windows.join("; ")
}

/// Coarse 25% buckets keep the system prompt stable across ordinary turns.
pub fn format_for_prompt(snapshot: Option<&UsageSnapshot>) -> String {
    let Some(snapshot) = snapshot else {
        return "usage unavailable".into();
    };
    if snapshot.windows.is_empty() {
        let status = serde_json::to_value(snapshot.status).unwrap_or_default();
        return format!("usage {}", status.as_str().unwrap_or("error"));
    }
    let windows: Vec<String> = snapshot
        .windows
        .iter()
        .map(|w| {
            let used = w.used_percent;
            let bucket = match () {
                _ if used >= 100.0 => match w.reset_at {
                    Some(reset) => format!("exhausted until {}", iso_minute(reset)),
                    None => "exhausted, reset unknown".into(),
                },
                _ if used < 25.0 => "<25%".into(),
                _ if used < 50.0 => "25-50%".into(),
                _ if used < 75.0 => "50-75%".into(),
                _ => "75-100%".into(),
            };
            format!("{} {bucket}", w.name)
        })
        .collect();
    windows.join("; ")
}
