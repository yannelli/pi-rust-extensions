mod common;

use common::now_ms;
use common::routing::*;
use pi_ext_subscription_router::config::{ModelRef, Strategy, parse_config};
use pi_ext_subscription_router::router::{Reason, RouteInput};
use pi_ext_subscription_router::selection::{QuotaScore, score_quota, select_candidate};
use serde_json::json;

#[test]
fn invalid_configs_are_rejected_whole() {
    let bad = [
        (
            json!({ "profiles": { "Main": { "chain": [{ "model": "a/b" }] } } }),
            "id must match",
        ),
        (
            json!({ "profiles": { "m": { "chain": [] } } }),
            "non-empty array",
        ),
        (
            json!({ "profiles": { "m": { "chain": [{ "model": "router/x" }] } } }),
            "another router model",
        ),
        (
            json!({ "profiles": { "m": { "chain": [{ "model": "a/b" }, { "model": "a/b" }] } } }),
            "listed twice",
        ),
        (
            json!({ "profiles": { "m": { "chain": [{ "model": "a/b", "thinking": "huge" }] } } }),
            "thinking",
        ),
        (
            json!({ "profiles": { "m": { "chain": [{ "model": "a" }] } } }),
            "provider/model-id",
        ),
        (
            json!({ "profiles": { "m": { "strategy": "random", "chain": [{ "model": "a/b" }] } } }),
            "strategy",
        ),
        (
            json!({ "profiles": { "m": { "maxWaitMinutes": 0, "chain": [{ "model": "a/b" }] } } }),
            "maxWaitMinutes",
        ),
        (json!({ "profiles": [] }), "profiles"),
        (json!({ "autoSwitchOn429": "yes" }), "autoSwitchOn429"),
    ];
    for (config, expected) in bad {
        let error = parse_config(&config).unwrap_err();
        assert!(
            error.contains(expected),
            "{error} should mention {expected}"
        );
    }
    let ok = parse_config(&json!({ "autoSwitchOn429": false, "profiles": {} })).unwrap();
    assert!(!ok.auto_switch_on_429);
    let p = two_chain(
        "balanced",
        json!({ "exhaustionPolicy": "fail", "maxWaitMinutes": 30 }),
    );
    assert_eq!(p.name, "Router main");
    assert_eq!(p.strategy(), Strategy::Balanced);
    assert_eq!(p.exhaustion.unwrap().max_wait_ms(), Some(1_800_000));
}

#[test]
fn balanced_keeps_incumbent_unless_twenty_percent_better() {
    let measured = |score: f64| QuotaScore::Measured {
        score,
        window: "7d".into(),
        roundoff: Some(0.0),
        stale: None,
    };
    let scores = [measured(0.5), measured(0.59)];
    assert_eq!(
        select_candidate(&scores, Strategy::Balanced, Some(0)),
        Some(0)
    );
    assert_eq!(
        select_candidate(
            &[measured(0.5), measured(0.61)],
            Strategy::Balanced,
            Some(0)
        ),
        Some(1)
    );
    assert_eq!(
        select_candidate(&[measured(0.5), measured(0.6)], Strategy::Balanced, Some(0)),
        Some(0)
    );
    assert_eq!(select_candidate(&scores, Strategy::Balanced, None), Some(1));
    assert_eq!(select_candidate(&scores, Strategy::Ordered, None), Some(0));
    let fallback = QuotaScore::Fallback { reason: "x".into() };
    assert_eq!(
        select_candidate(
            &[fallback.clone(), measured(0.1)],
            Strategy::Balanced,
            Some(0)
        ),
        Some(1)
    );
    assert_eq!(
        select_candidate(&[fallback], Strategy::Balanced, Some(0)),
        Some(0)
    );
    assert_eq!(select_candidate(&[], Strategy::Balanced, None), None);
}

#[test]
fn scores_remaining_quota_per_time_left() {
    let model = ModelRef::new("openai-codex", "gpt-5.5");
    let half_window = snapshot(
        "openai-codex",
        vec![window("7d", 50.0, 302_400_000, 604_800_000)],
    );
    let QuotaScore::Measured { score, window, .. } =
        score_quota(&model, Some(&half_window), now_ms())
    else {
        panic!("measured");
    };
    assert!((score - 1.0).abs() < 0.01, "{score}");
    assert_eq!(window, "7d");
    assert!(matches!(
        score_quota(&model, None, now_ms()),
        QuotaScore::Fallback { .. }
    ));
}

#[test]
fn parses_route_requests() {
    let request = json!({
        "model": { "provider": "router", "id": "main" }, "thinkingLevel": "low", "reason": "retry",
        "previous": { "model": { "provider": "anthropic", "id": "a" } },
        "failed": { "model": { "provider": "openai-codex", "id": "gpt" }, "message": {
            "role": "assistant", "stopReason": "error", "errorMessage": "Rate limit reached", "content": [], "timestamp": 5 } }
    });
    let parsed = RouteInput::from_json(&request);
    assert_eq!(parsed.reason, Reason::Retry);
    assert_eq!(parsed.thinking_level, "low");
    assert_eq!(parsed.previous.unwrap().key(), "anthropic/a");
    let failed = parsed.failed.unwrap();
    assert!(failed.quota && !failed.partial);
    assert_eq!(failed.timestamp, 5);
}
