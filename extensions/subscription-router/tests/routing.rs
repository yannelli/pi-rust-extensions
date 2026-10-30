mod common;

use common::now_ms;
use common::routing::*;
use pi_ext_subscription_router::config::{ModelRef, parse_config};
use pi_ext_subscription_router::router::{
    Admission, Pick, Reason, RouteError, can_wait, route_wait,
};
use pi_ext_subscription_router::usage::{Scope, UsageDecision};
use serde_json::json;

#[tokio::test]
async fn ordered_and_balanced_picks() {
    let router = router();
    let env = Env::new(&BOTH, None);
    let ordered = router
        .route(
            &two_chain("ordered", json!({})),
            &input(Reason::User, None, None),
            &env,
        )
        .await
        .unwrap();
    assert_eq!(ordered.entry.model.key(), "openai-codex/gpt-5.5");
    assert_eq!(ordered.thinking_level, "medium");
    assert!(ordered.decision.is_some());
    router.usage.insert(snapshot(
        "openai-codex",
        vec![window("7d", 90.0, 500_000_000, 604_800_000)],
    ));
    router.usage.insert(snapshot(
        "anthropic",
        vec![window("seven_day", 10.0, 500_000_000, 604_800_000)],
    ));
    let balanced = router
        .route(
            &two_chain("balanced", json!({})),
            &input(Reason::User, None, None),
            &env,
        )
        .await
        .unwrap();
    assert_eq!(balanced.entry.model.key(), "anthropic/claude-sonnet-5");
    assert_eq!(balanced.thinking_level, "high");
    let missing = Env::new(&["anthropic/claude-sonnet-5"], None);
    let picked = router.pick(&two_chain("ordered", json!({})), &missing, None);
    let Pick::Chosen { entry, blocked, .. } = picked else {
        panic!("chosen")
    };
    assert_eq!(entry.model.key(), "anthropic/claude-sonnet-5");
    assert!(!blocked[0].quota);
}

#[tokio::test]
async fn retry_after_confirmed_quota_moves_down_the_chain() {
    let router = router();
    let block = UsageDecision {
        block: true,
        window: Some("5h".into()),
        reset_at: Some(now_ms() + 3_600_000),
        scope: Some(Scope::Provider),
        ..Default::default()
    };
    let env = Env::new(&BOTH, Some(block));
    let failed_at = now_ms() - 1_000;
    let request = input(
        Reason::Retry,
        Some(quota_failure("openai-codex/gpt-5.5", failed_at, false)),
        None,
    );
    let routed = router
        .route(&two_chain("ordered", json!({})), &request, &env)
        .await
        .unwrap();
    assert_eq!(routed.entry.model.key(), "anthropic/claude-sonnet-5");
    assert_eq!(
        env.reads.lock().unwrap().as_slice(),
        [("openai-codex/gpt-5.5".to_owned(), failed_at)]
    );
    assert!(
        router
            .blocked_by(&ModelRef::new("openai-codex", "gpt-5.5"))
            .is_some()
    );
    let next = router
        .route(
            &two_chain("ordered", json!({})),
            &input(Reason::User, None, None),
            &env,
        )
        .await
        .unwrap();
    assert_eq!(next.entry.model.key(), "anthropic/claude-sonnet-5");
}

#[tokio::test]
async fn unconfirmed_or_partial_failures_do_not_block() {
    let router = router();
    let unconfirmed = Env::new(&BOTH, Some(UsageDecision::default()));
    let request = input(
        Reason::Retry,
        Some(quota_failure("openai-codex/gpt-5.5", now_ms(), false)),
        None,
    );
    let routed = router
        .route(&two_chain("ordered", json!({})), &request, &unconfirmed)
        .await
        .unwrap();
    assert_eq!(routed.entry.model.key(), "openai-codex/gpt-5.5");
    assert!(routed.decision.is_none());
    assert!(router.tracker().active(now_ms()).is_empty());
    let no_read = Env::new(&BOTH, None);
    router
        .route(&two_chain("ordered", json!({})), &request, &no_read)
        .await
        .unwrap();
    assert!(router.tracker().active(now_ms()).is_empty());
    let block = UsageDecision {
        block: true,
        ..Default::default()
    };
    let partial = input(
        Reason::Retry,
        Some(quota_failure("openai-codex/gpt-5.5", now_ms(), true)),
        None,
    );
    let error = router
        .route(
            &two_chain("ordered", json!({})),
            &partial,
            &Env::new(&BOTH, Some(block)),
        )
        .await;
    assert_eq!(error.unwrap_err(), RouteError::Partial);
    let mark = &router.tracker().active(now_ms())[0];
    assert!(!mark.reset_known);
    assert!(mark.until - now_ms() <= 5 * 60_000);
}

#[tokio::test]
async fn exhausted_chains_report_reset_and_policy() {
    let router = router();
    let reset = now_ms() + 2 * 3_600_000;
    let block = UsageDecision {
        block: true,
        window: Some("7d".into()),
        reset_at: Some(reset),
        scope: Some(Scope::Provider),
        ..Default::default()
    };
    let env = Env::new(&BOTH, Some(block));
    for model in BOTH {
        let mark = UsageDecision {
            block: true,
            window: Some("7d".into()),
            reset_at: Some(reset),
            scope: Some(Scope::Provider),
            ..Default::default()
        };
        router
            .tracker()
            .mark(&ModelRef::parse(model).unwrap(), &mark, now_ms());
    }
    let wait = two_chain(
        "ordered",
        json!({ "exhaustionPolicy": "wait", "maxWaitMinutes": 180 }),
    );
    let error = router
        .route(&wait, &input(Reason::User, None, None), &env)
        .await
        .unwrap_err();
    let RouteError::Exhausted { message, reset_at } = error else {
        panic!("exhausted")
    };
    assert_eq!(reset_at, Some(reset));
    assert!(
        message.starts_with("router/main: every model is unavailable"),
        "{message}"
    );
    assert!(can_wait(wait.exhaustion.as_ref(), reset_at, now_ms()));
    let short = two_chain("ordered", json!({ "maxWaitMinutes": 60 }));
    assert!(!can_wait(short.exhaustion.as_ref(), reset_at, now_ms()));
    let fail = two_chain("ordered", json!({ "exhaustionPolicy": "fail" }));
    assert!(!can_wait(fail.exhaustion.as_ref(), reset_at, now_ms()));
    assert_eq!(
        route_wait(wait.exhaustion.as_ref(), reset_at, now_ms()),
        Some(reset)
    );
    assert_eq!(
        route_wait(short.exhaustion.as_ref(), reset_at, now_ms()),
        None
    );
    assert_eq!(
        route_wait(fail.exhaustion.as_ref(), reset_at, now_ms()),
        None
    );
    let unlimited = two_chain("ordered", json!({ "exhaustionPolicy": "wait" }));
    assert_eq!(
        route_wait(unlimited.exhaustion.as_ref(), reset_at, now_ms()),
        None
    );
    let default = two_chain("ordered", json!({}));
    assert_eq!(
        route_wait(default.exhaustion.as_ref(), reset_at, now_ms()),
        None
    );
    let config = parse_config(&json!({ "profiles": { "main": { "chain": [{ "model": "openai-codex/gpt-5.5" }], "exhaustionPolicy": "fail" } } })).unwrap();
    let admission = router.admit("router/main", &config, &env);
    assert_eq!(
        admission,
        Admission {
            ok: false,
            resolved_model: None,
            reset_at: Some(reset),
            wait: Some(false)
        }
    );
    assert!(router.admit("mock/m", &config, &env).ok);
    router.tracker().clear();
    assert_eq!(
        router
            .admit("router/main", &config, &env)
            .resolved_model
            .as_deref(),
        Some("openai-codex/gpt-5.5")
    );
}

#[tokio::test]
async fn continuation_keeps_previous_model() {
    let router = router();
    let env = Env::new(&BOTH, None);
    let request = input(
        Reason::Continuation,
        None,
        Some("anthropic/claude-sonnet-5"),
    );
    let routed = router
        .route(&two_chain("ordered", json!({})), &request, &env)
        .await
        .unwrap();
    assert_eq!(routed.entry.model.key(), "anthropic/claude-sonnet-5");
    assert!(routed.decision.is_none());
}
