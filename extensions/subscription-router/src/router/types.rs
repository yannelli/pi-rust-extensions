//! Route request parsing, decisions, admission answers, and the quota error rules.

use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};
use serde::Serialize;
use serde_json::Value;

use crate::config::{ChainEntry, Exhaustion, ModelRef, Policy};
use crate::fsutil::iso_seconds;
use crate::selection::{QuotaScore, format_score};

static QUOTA_ERROR: LazyLock<Regex> = LazyLock::new(|| {
    let pattern = r"\b429\b|quota|rate.?limit|too many requests|usage.?limit|billing|out of budget|extra.?usage|available balance|UsageLimitError|subscription_sharing_usage_limit_exceeded";
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .expect("valid regex")
});

/// Whether an assistant error text could be subscription quota.
pub fn is_quota_error(stop_reason: &str, error_message: Option<&str>) -> bool {
    stop_reason == "error" && QUOTA_ERROR.is_match(error_message.unwrap_or_default())
}

/// A failed stream with text, thinking, or a tool call cannot be replayed without duplicates.
pub fn has_partial_output(message: &Value) -> bool {
    let content = message.get("content").and_then(Value::as_array);
    content.into_iter().flatten().any(|part| {
        let text = |key: &str| {
            part.get(key)
                .and_then(Value::as_str)
                .is_some_and(|t| !t.is_empty())
        };
        match part.get("type").and_then(Value::as_str) {
            Some("toolCall") => true,
            Some("text") => text("text"),
            Some("thinking") => text("thinking"),
            _ => false,
        }
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Reason {
    User,
    Continuation,
    Retry,
    Direct,
}

impl Reason {
    pub fn parse(text: &str) -> Self {
        match text {
            "continuation" => Self::Continuation,
            "retry" => Self::Retry,
            "direct" => Self::Direct,
            _ => Self::User,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FailedRequest {
    pub model: ModelRef,
    pub quota: bool,
    pub partial: bool,
    pub timestamp: i64,
}

impl FailedRequest {
    pub fn from_message(model: ModelRef, message: &Value) -> Self {
        let stop = message
            .get("stopReason")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let error = message.get("errorMessage").and_then(Value::as_str);
        Self {
            model,
            quota: is_quota_error(stop, error),
            partial: has_partial_output(message),
            timestamp: message
                .get("timestamp")
                .and_then(Value::as_i64)
                .unwrap_or(0),
        }
    }
}

/// The parts of `ModelRouteRequest::to_json` the router reads.
#[derive(Clone, Debug, PartialEq)]
pub struct RouteInput {
    pub reason: Reason,
    pub thinking_level: String,
    pub previous: Option<ModelRef>,
    pub failed: Option<FailedRequest>,
}

fn model_of(value: &Value) -> Option<ModelRef> {
    let model = value.get("model")?;
    let provider = model.get("provider")?.as_str()?;
    let id = model.get("id")?.as_str()?;
    Some(ModelRef::new(provider, id))
}

impl RouteInput {
    pub fn from_json(request: &Value) -> Self {
        let failed = request.get("failed").and_then(|failed| {
            let model = model_of(failed)?;
            Some(FailedRequest::from_message(model, failed.get("message")?))
        });
        Self {
            reason: Reason::parse(
                request
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("user"),
            ),
            thinking_level: request
                .get("thinkingLevel")
                .and_then(Value::as_str)
                .unwrap_or("medium")
                .to_owned(),
            previous: request.get("previous").and_then(model_of),
            failed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionCandidate {
    pub model: String,
    pub quota: QuotaScore,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionBlocked {
    pub model: String,
    pub reason: String,
}

/// One scored pick, stored as a `router:decision` session entry.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RouteDecision {
    pub at: i64,
    pub profile: String,
    pub strategy: String,
    pub reason: Reason,
    pub chosen: String,
    pub candidates: Vec<DecisionCandidate>,
    pub blocked: Vec<DecisionBlocked>,
}

pub fn format_decision(decision: &RouteDecision) -> String {
    let reason = serde_json::to_value(decision.reason).unwrap_or_default();
    let mut lines = vec![format!(
        "{} router/{} {} -> {} ({})",
        iso_seconds(decision.at),
        decision.profile,
        reason.as_str().unwrap_or_default(),
        decision.chosen,
        decision.strategy
    )];
    for c in &decision.candidates {
        let age = c
            .age_ms
            .map_or("no data".into(), |ms| format!("data {}m old", ms / 60_000));
        lines.push(format!(
            "    {}: {}, {age}",
            c.model,
            format_score(&c.quota)
        ));
    }
    for b in &decision.blocked {
        lines.push(format!("    {}: {}", b.model, b.reason));
    }
    lines.join("\n")
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ineligible {
    pub entry: ChainEntry,
    pub reason: String,
    pub reset_at: Option<i64>,
    /// Blocked by exhausted quota, as opposed to an unknown model or missing credentials.
    pub quota: bool,
}

/// The answer to `router:rpc:admit`.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Admission {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait: Option<bool>,
}

/// Whether a refused request may wait for `reset_at` (default policy: wait, no limit).
pub fn can_wait(exhaustion: Option<&Exhaustion>, reset_at: Option<i64>, now: i64) -> bool {
    let Some(reset) = reset_at else { return false };
    if exhaustion.is_some_and(|e| e.policy == Policy::Fail) {
        return false;
    }
    exhaustion
        .and_then(Exhaustion::max_wait_ms)
        .is_none_or(|max| reset - now <= max)
}

/// The reset `virtualModel.route` sleeps until: only with policy `wait`, a configured
/// `maxWaitMinutes`, and a reset within it. Otherwise the route fails with the exhausted error.
pub fn route_wait(exhaustion: Option<&Exhaustion>, reset_at: Option<i64>, now: i64) -> Option<i64> {
    let exhaustion = exhaustion.filter(|e| e.policy == Policy::Wait)?;
    let (reset, max) = (reset_at?, exhaustion.max_wait_ms()?);
    (reset - now <= max).then_some(reset)
}

pub fn exhausted_message(profile: &str, blocked: &[Ineligible], reset_at: Option<i64>) -> String {
    let when = reset_at.map_or("an unknown time".into(), iso_seconds);
    let detail: Vec<String> = blocked
        .iter()
        .map(|b| format!("{}: {}", b.entry.model.key(), b.reason))
        .collect();
    format!(
        "router/{profile}: every model is unavailable ({}). Earliest reset {when}.",
        detail.join("; ")
    )
}
