//! Parsers for the Anthropic `/api/oauth/usage` and ChatGPT Codex `wham/usage` payloads.

use base64::Engine;
use serde_json::{Map, Value};

use super::{Credits, FIVE_HOURS_MS, SEVEN_DAYS_MS, Scope, UsageWindow};

/// Numeric resets below this are Unix seconds, above it milliseconds.
const SECONDS_VS_MS: f64 = 100_000_000_000.0;
const CODEX_JWT_CLAIM: &str = "https://api.openai.com/auth";

pub struct Parsed {
    pub windows: Vec<UsageWindow>,
    pub credits: Option<Credits>,
    pub tier: Option<String>,
}

fn timestamp(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::String(text) => chrono::DateTime::parse_from_rfc3339(text)
            .or_else(|_| chrono::DateTime::parse_from_rfc2822(text))
            .ok()
            .map(|t| t.timestamp_millis()),
        Value::Number(n) => {
            let raw = n.as_f64().filter(|v| v.is_finite())?;
            Some(if raw < SECONDS_VS_MS {
                raw * 1000.0
            } else {
                raw
            } as i64)
        }
        _ => None,
    }
}

fn percentage(value: Option<&Value>) -> Option<f64> {
    let raw = value?.as_f64().filter(|v| v.is_finite() && *v >= 0.0)?;
    Some(raw.min(100.0))
}

pub fn parse_anthropic(payload: &Value) -> Option<Parsed> {
    let root = payload.as_object()?;
    let specs: [(&str, Option<&str>); 4] = [
        ("five_hour", None),
        ("seven_day", None),
        ("seven_day_opus", Some("opus")),
        ("seven_day_sonnet", Some("sonnet")),
    ];
    let windows: Vec<UsageWindow> = specs
        .iter()
        .filter_map(|(key, family)| {
            let item = root.get(*key)?.as_object()?;
            let used_percent = percentage(item.get("utilization").or(item.get("used_percentage")))?;
            Some(UsageWindow {
                name: (*key).to_owned(),
                used_percent,
                reset_at: timestamp(item.get("resets_at").or(item.get("reset_at"))),
                duration_ms: Some(if *key == "five_hour" {
                    FIVE_HOURS_MS
                } else {
                    SEVEN_DAYS_MS
                }),
                span_source: Some("reported".into()),
                scope: if family.is_some() {
                    Scope::Model
                } else {
                    Scope::Provider
                },
                families: family.map(|f| vec![f.to_owned()]),
            })
        })
        .collect();
    let tier = root
        .get("plan_type")
        .and_then(Value::as_str)
        .map(str::to_owned);
    (!windows.is_empty()).then_some(Parsed {
        windows,
        credits: None,
        tier,
    })
}

fn codex_reset(item: &Map<String, Value>, now: i64) -> Option<i64> {
    timestamp(item.get("reset_at")).or_else(|| {
        let after = item
            .get("reset_after_seconds")?
            .as_f64()
            .filter(|s| *s >= 0.0)?;
        now.checked_add((after * 1000.0) as i64)
    })
}

pub fn parse_codex(payload: &Value, now: i64) -> Option<Parsed> {
    let root = payload.as_object()?;
    let limits = root.get("rate_limit")?.as_object()?;
    let positions = [
        ("primary_window", "5h", FIVE_HOURS_MS),
        ("secondary_window", "7d", SEVEN_DAYS_MS),
    ];
    let windows: Vec<UsageWindow> = positions
        .iter()
        .filter_map(|(key, fallback, fallback_ms)| {
            let item = limits.get(*key)?.as_object()?;
            let used_percent = percentage(item.get("used_percent"))?;
            let seconds = item
                .get("limit_window_seconds")
                .and_then(Value::as_i64)
                .filter(|s| *s > 0);
            let name = match seconds {
                Some(604_800) => "7d".to_owned(),
                Some(18_000) => "5h".to_owned(),
                Some(s) => format!("{s}s"),
                None => (*fallback).to_owned(),
            };
            Some(UsageWindow {
                name,
                used_percent,
                reset_at: codex_reset(item, now),
                duration_ms: Some(seconds.map_or(*fallback_ms, |s| s * 1000)),
                span_source: Some(
                    if seconds.is_some() {
                        "reported"
                    } else {
                        "assumed"
                    }
                    .into(),
                ),
                scope: Scope::Provider,
                families: None,
            })
        })
        .collect();
    if windows.is_empty() {
        return None;
    }
    let credits = root
        .get("credits")
        .and_then(Value::as_object)
        .map(|c| Credits {
            available: c.get("has_credits") == Some(&Value::Bool(true)),
            unlimited: c.get("unlimited") == Some(&Value::Bool(true)),
            balance: c.get("balance").and_then(Value::as_str).map(str::to_owned),
        });
    let tier = ["plan_type", "tier"]
        .iter()
        .find_map(|k| root.get(*k).and_then(Value::as_str))
        .map(str::to_owned);
    Some(Parsed {
        windows,
        credits,
        tier,
    })
}

pub fn jwt_payload(token: &str) -> Option<Value> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let segment = parts[1].trim_end_matches('=');
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(segment)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(segment))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// `chatgpt_account_id` from a Codex access-token JWT.
pub fn codex_account_id(token: &str) -> Option<String> {
    let payload = jwt_payload(token)?;
    let id = payload
        .get(CODEX_JWT_CLAIM)?
        .get("chatgpt_account_id")?
        .as_str()?;
    (!id.is_empty()).then(|| id.to_owned())
}
