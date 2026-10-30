mod common;

use std::collections::HashSet;

use common::now_ms;
use pi_ext_subscription_router::config::ModelRef;
use pi_ext_subscription_router::failover::{exhausted_message, failed_slot, retry_message};
use pi_ext_subscription_router::family::Family;
use pi_ext_subscription_router::providers::provider_config;
use pi_ext_subscription_router::slots::{
    SlotInfo, failover_order, select_fresh, status_line, status_text,
};
use pi_ext_subscription_router::usage::{Scope, Status, UsageSnapshot, UsageWindow};
use serde_json::json;

fn window(name: &str, used: f64, reset_in: i64, duration: i64) -> UsageWindow {
    UsageWindow {
        name: name.into(),
        used_percent: used,
        reset_at: Some(now_ms() + reset_in),
        duration_ms: Some(duration),
        span_source: None,
        scope: Scope::Provider,
        families: None,
    }
}

fn slot(slot: u32, five_hour: f64, seven_day: f64, age_ms: i64) -> SlotInfo {
    let snapshot = UsageSnapshot {
        provider: Family::Codex.provider(slot),
        status: Status::Available,
        windows: vec![
            window("5h", five_hour, 3_600_000, 18_000_000),
            window("7d", seven_day, 2 * 86_400_000, 604_800_000),
        ],
        credits: None,
        tier: Some("pro".into()),
        retrieved_at: now_ms() - age_ms,
        expires_at: now_ms() - age_ms + 300_000,
        message: None,
    };
    SlotInfo {
        family: Family::Codex,
        slot,
        snapshot: Some(snapshot),
    }
}

#[test]
fn fresh_selection_prefers_most_quota_then_current_then_lowest() {
    let slots = [
        slot(1, 10.0, 60.0, 0),
        slot(2, 10.0, 20.0, 0),
        slot(3, 10.0, 20.0, 0),
    ];
    assert_eq!(select_fresh(&slots, None, now_ms()), Some(2));
    assert_eq!(select_fresh(&slots, Some(3), now_ms()), Some(3));
    let limited = [slot(1, 100.0, 0.0, 0), slot(2, 10.0, 50.0, 0)];
    assert_eq!(select_fresh(&limited, Some(1), now_ms()), Some(2));
    let stale = [slot(1, 10.0, 0.0, 600_000), slot(2, 10.0, 90.0, 0)];
    assert_eq!(select_fresh(&stale, None, now_ms()), Some(2));
    let unknown = SlotInfo {
        family: Family::Codex,
        slot: 4,
        snapshot: None,
    };
    assert_eq!(select_fresh(&[unknown], None, now_ms()), None);
}

#[test]
fn failover_order_skips_tried_and_limited_slots() {
    let unknown = SlotInfo {
        family: Family::Codex,
        slot: 4,
        snapshot: None,
    };
    let slots = [
        slot(1, 10.0, 70.0, 0),
        slot(2, 100.0, 0.0, 0),
        slot(3, 10.0, 30.0, 0),
        unknown,
    ];
    let tried = HashSet::from([3]);
    assert_eq!(failover_order(&slots, &tried, now_ms()), [1, 4]);
    assert_eq!(failover_order(&slots, &HashSet::new(), now_ms()), [3, 1, 4]);
}

#[test]
fn status_text_shows_slot_percent_and_reset() {
    let text = status_text(&slot(2, 10.0, 37.0, 0), now_ms());
    assert!(text.starts_with("Codex #2 · 63% · 7d 2d"), "{text}");
    let text = status_text(&slot(2, 90.0, 37.0, 0), now_ms());
    assert!(text.starts_with("Codex #2 · 63% · 5h 1h"), "{text}");
    assert!(status_text(&slot(1, 100.0, 1.0, 0), now_ms()).starts_with("Codex #1 · 5h limit · 1h"));
    assert_eq!(
        status_text(&slot(1, 1.0, 1.0, 600_000), now_ms()),
        "Codex #1 · stale"
    );
    let none = SlotInfo {
        family: Family::Anthropic,
        slot: 2,
        snapshot: None,
    };
    assert_eq!(status_text(&none, now_ms()), "Claude #2 · unavailable");
    let line = status_line(&slot(2, 10.0, 37.0, 0), now_ms());
    assert!(
        line.starts_with(
            "OpenAI Codex #2 (pro): 63% of the seven-day quota remaining; seven-day reset in 2d"
        ),
        "{line}"
    );
}

#[test]
fn detects_429_on_the_current_slot() {
    let message = json!({ "role": "assistant", "stopReason": "error", "provider": "openai-codex-2", "model": "gpt-5.5", "errorMessage": "Too many requests" });
    let current = ModelRef::new("openai-codex-2", "gpt-5.5");
    assert_eq!(
        failed_slot(&message, Some(&current), Some(429)),
        Some((Family::Codex, 2, "gpt-5.5".into()))
    );
    assert_eq!(failed_slot(&message, Some(&current), None), None);
    let with_code = json!({ "role": "assistant", "stopReason": "error", "provider": "anthropic", "model": "c", "errorMessage": "429 rate_limit_error" });
    let current_anthropic = ModelRef::new("anthropic", "c");
    assert_eq!(
        failed_slot(&with_code, Some(&current_anthropic), None),
        Some((Family::Anthropic, 1, "c".into()))
    );
    let other = ModelRef::new("router", "main");
    assert_eq!(failed_slot(&message, Some(&other), Some(429)), None);
    let rewritten = retry_message(&message);
    assert_eq!(rewritten["errorMessage"], "HTTP 429: Too many requests");
    let stopped = exhausted_message(&message, Family::Codex);
    assert!(
        stopped["errorMessage"]
            .as_str()
            .unwrap()
            .starts_with("OpenAI Codex quota exceeded")
    );
}

#[test]
fn slot_provider_config_copies_base_models() {
    let base = vec![
        json!({ "id": "gpt-5.5", "name": "GPT-5.5", "provider": "openai-codex", "api": "openai-codex-responses", "baseUrl": "https://chatgpt.com/backend-api", "contextWindow": 400000 }),
        json!({ "id": "gpt-5.5-mini", "provider": "openai-codex", "api": "openai-codex-responses", "baseUrl": "https://chatgpt.com/backend-api" }),
    ];
    let config = provider_config(Family::Codex, 3, &base, "token-3").unwrap();
    assert_eq!(config["name"], "OpenAI Codex #3");
    assert_eq!(config["api"], "openai-codex-responses");
    assert_eq!(config["baseUrl"], "https://chatgpt.com/backend-api");
    assert_eq!(config["apiKey"], "token-3");
    assert_eq!(config["models"][1]["provider"], "openai-codex-3");
    assert_eq!(config["models"][0]["contextWindow"], 400000);
    assert!(config.get("oauth").is_none());
    assert!(provider_config(Family::Codex, 2, &[], "t").is_none());
}
