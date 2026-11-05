//! Accounts behind one provider: the policies that pick an account, the config of policies and
//! account chain entries, and the protocol with a pi that has `providerAuth`.

mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use common::http::MockHttp;
use common::{TestHost, ctx, now_ms, temp_dir};
use pi_ext_subscription_router::accounts::Account;
use pi_ext_subscription_router::config::{ModelRef, config_json, parse_config};
use pi_ext_subscription_router::family::Family;
use pi_ext_subscription_router::pick::{all_full, choose, full_until};
use pi_ext_subscription_router::state::{State, lock};
use pi_ext_subscription_router::usage::{Scope, Status, Urls, UsageSnapshot, UsageWindow};
use serde_json::{Value, json};

const HOUR: i64 = 3_600_000;

fn window(
    name: &str,
    used: f64,
    reset_in: i64,
    duration: i64,
    families: Option<&str>,
) -> UsageWindow {
    UsageWindow {
        name: name.into(),
        used_percent: used,
        reset_at: Some(now_ms() + reset_in),
        duration_ms: Some(duration),
        span_source: None,
        scope: if families.is_some() {
            Scope::Model
        } else {
            Scope::Provider
        },
        families: families.map(|f| vec![f.to_owned()]),
    }
}

/// Usage of an Anthropic account: five-hour and weekly use, plus an optional opus window.
fn usage(slot: u32, five_hour: f64, week: f64, opus: Option<f64>) -> UsageSnapshot {
    let mut windows = vec![
        window("5h", five_hour, 2 * HOUR, 5 * HOUR, None),
        window("7d", week, 72 * HOUR, 168 * HOUR, None),
    ];
    if let Some(used) = opus {
        windows.push(window("7d opus", used, 72 * HOUR, 168 * HOUR, Some("opus")));
    }
    UsageSnapshot {
        provider: Family::Anthropic.provider(slot),
        status: Status::Available,
        windows,
        credits: None,
        tier: Some("max".into()),
        retrieved_at: now_ms(),
        expires_at: now_ms() + 300_000,
        message: None,
    }
}

async fn state(name: &str, accounts: &[u32], config: Value) -> Arc<State> {
    let dir = temp_dir(name);
    let urls = Urls {
        codex: "http://127.0.0.1:9/wham/usage".into(),
        anthropic: "http://127.0.0.1:9/api/oauth/usage".into(),
    };
    let state = Arc::new(State::with_urls(&dir, urls));
    for slot in accounts {
        state
            .accounts
            .upsert(Account::new(Family::Anthropic, *slot, None))
            .await
            .unwrap();
    }
    *lock(&state.config) = parse_config(&config).unwrap();
    state.accounts_mode.store(true, Ordering::Relaxed);
    state
}

const SONNET: &str = "claude-sonnet-5";

#[tokio::test]
async fn balance_takes_the_account_with_most_room_and_stays_unless_another_is_much_better() {
    let state = state(
        "balance",
        &[2, 3],
        json!({ "accounts": { "anthropic": { "strategy": "balance" } } }),
    )
    .await;
    state.usage().insert(usage(1, 80.0, 70.0, None));
    state.usage().insert(usage(2, 10.0, 20.0, None));
    state.usage().insert(usage(3, 40.0, 30.0, None));
    assert_eq!(choose(&state, Family::Anthropic, SONNET, now_ms()), 2);
    // Account 3 gains a little; account 2 stays.
    state.usage().insert(usage(2, 30.0, 25.0, None));
    state.usage().insert(usage(3, 25.0, 25.0, None));
    assert_eq!(choose(&state, Family::Anthropic, SONNET, now_ms()), 2);
    // Account 2 is full: account 3.
    state.usage().insert(usage(2, 100.0, 40.0, None));
    assert_eq!(choose(&state, Family::Anthropic, SONNET, now_ms()), 3);
}

#[tokio::test]
async fn fill_stays_until_full_and_threshold_switches_at_its_percent() {
    let fill = state(
        "fill",
        &[2],
        json!({ "accounts": { "anthropic": { "strategy": "fill" } } }),
    )
    .await;
    fill.usage().insert(usage(1, 95.0, 50.0, None));
    fill.usage().insert(usage(2, 0.0, 0.0, None));
    assert_eq!(
        choose(&fill, Family::Anthropic, SONNET, now_ms()),
        1,
        "95% is not full"
    );
    fill.usage().insert(usage(1, 100.0, 50.0, None));
    assert_eq!(choose(&fill, Family::Anthropic, SONNET, now_ms()), 2);
    fill.usage().insert(usage(1, 0.0, 0.0, None));
    assert_eq!(
        choose(&fill, Family::Anthropic, SONNET, now_ms()),
        2,
        "fill stays on the new account"
    );

    let config =
        json!({ "accounts": { "anthropic": { "strategy": "threshold", "switchAt": 90 } } });
    let threshold = state("threshold", &[2], config).await;
    threshold.usage().insert(usage(1, 91.0, 50.0, None));
    threshold.usage().insert(usage(2, 0.0, 0.0, None));
    assert_eq!(choose(&threshold, Family::Anthropic, SONNET, now_ms()), 2);
}

#[tokio::test]
async fn manual_pins_and_routes_decide_before_the_policy() {
    let config = json!({ "accounts": { "anthropic": { "strategy": "manual", "account": 3 } } });
    let state = state("manual", &[2, 3], config).await;
    assert_eq!(choose(&state, Family::Anthropic, SONNET, now_ms()), 3);
    lock(&state.picks).pinned.insert(Family::Anthropic, 2);
    assert_eq!(choose(&state, Family::Anthropic, SONNET, now_ms()), 2);
    lock(&state.picks).routed = Some((Family::Anthropic, SONNET.into(), 1));
    assert_eq!(choose(&state, Family::Anthropic, SONNET, now_ms()), 1);
    assert_eq!(
        choose(&state, Family::Anthropic, "claude-opus-5-5", now_ms()),
        2,
        "routes are per model"
    );
}

#[tokio::test]
async fn model_windows_make_an_account_full_only_for_their_models() {
    let state = state("windows", &[2], json!({ "accounts": {} })).await;
    state.usage().insert(usage(1, 10.0, 10.0, Some(100.0)));
    state.usage().insert(usage(2, 100.0, 50.0, None));
    let now = now_ms();
    assert!(full_until(&state, Family::Anthropic, 1, "claude-opus-5-5", 100.0, now).is_some());
    assert!(full_until(&state, Family::Anthropic, 1, SONNET, 100.0, now).is_none());
    assert!(all_full(&state, Family::Anthropic, "claude-opus-5-5", now));
    assert!(!all_full(&state, Family::Anthropic, SONNET, now));
    assert_eq!(choose(&state, Family::Anthropic, SONNET, now), 1);
}

#[test]
fn config_reads_and_writes_policies_and_account_chain_entries() {
    let raw = json!({
        "accounts": {
            "anthropic": {
                "strategy": "threshold", "switchAt": 85,
                "fallback": [{ "model": "openai-codex/gpt-5.5", "thinking": "high" }],
                "returnWhenAvailable": false
            },
            "openai-codex": { "strategy": "fill", "useResets": "when-all-full", "resetMinWaitMinutes": 30 }
        },
        "profiles": { "main": { "chain": [
            { "model": "anthropic/claude-opus-5-5", "account": 2 },
            { "model": "anthropic-3/claude-opus-5-5" }
        ] } }
    });
    let config = parse_config(&raw).unwrap();
    let chain: Vec<String> = config.profiles[0]
        .chain
        .iter()
        .map(|e| e.model.key())
        .collect();
    assert_eq!(
        chain,
        ["anthropic-2/claude-opus-5-5", "anthropic-3/claude-opus-5-5"]
    );
    let anthropic = config.policy(Family::Anthropic);
    assert_eq!(
        (anthropic.full_at(), anthropic.return_when_available),
        (85.0, false)
    );
    assert_eq!(
        anthropic.fallback[0].model,
        ModelRef::new("openai-codex", "gpt-5.5")
    );
    let written = config_json(&config);
    assert_eq!(
        written["profiles"]["main"]["chain"],
        json!([
            { "model": "anthropic/claude-opus-5-5", "account": 2 },
            { "model": "anthropic/claude-opus-5-5", "account": 3 }
        ])
    );
    assert_eq!(
        written["accounts"]["openai-codex"],
        json!({ "strategy": "fill", "useResets": "when-all-full", "resetMinWaitMinutes": 30.0 })
    );
    assert_eq!(parse_config(&written).unwrap(), config);

    for (bad, error) in [
        (
            json!({ "accounts": { "anthropic": { "strategy": "random" } } }),
            "strategy",
        ),
        (
            json!({ "accounts": { "anthropic": { "switchAt": 0 } } }),
            "switchAt",
        ),
        (
            json!({ "accounts": { "openai": {} } }),
            "expected anthropic or openai-codex",
        ),
        (
            json!({ "accounts": { "anthropic": { "fallback": [{ "model": "anthropic/x" }] } } }),
            "itself",
        ),
        (
            json!({ "profiles": { "p": { "chain": [{ "model": "openai/gpt", "account": 2 }] } } }),
            "accounts",
        ),
    ] {
        let message = parse_config(&bad).unwrap_err();
        assert!(message.contains(error), "{message}");
    }
}

fn pi(method: &str, params: &Value) -> Value {
    match (method, params["method"].as_str()) {
        ("pi.call", Some("find")) => {
            json!({ "provider": params["args"][0], "id": params["args"][1], "api": "anthropic-messages" })
        }
        ("pi.call", _) => json!(true),
        ("usage.get", _) => json!([]),
        ("model.set", _) => json!(true),
        _ => Value::Null,
    }
}

#[tokio::test]
async fn with_provider_auth_accounts_are_logins_picked_per_request() {
    let mock = MockHttp::start(vec![]).await;
    let dir = temp_dir("provider-auth");
    std::fs::create_dir_all(dir.join("ext/subscription-router")).unwrap();
    let config = json!({ "accounts": { "anthropic": { "strategy": "fill" } } });
    std::fs::write(dir.join("ext/subscription-router.json"), config.to_string()).unwrap();
    let accounts = json!({ "version": 1, "accounts": { "anthropic-2": { "type": "pi" } } });
    std::fs::write(
        dir.join("ext/subscription-router/accounts.json"),
        accounts.to_string(),
    )
    .unwrap();
    let urls = Urls {
        codex: format!("{}/wham/usage", mock.url),
        anthropic: format!("{}/api/oauth/usage", mock.url),
    };
    let state = Arc::new(State::with_urls(&dir, urls));
    let mut host = TestHost::start(pi_ext_subscription_router::extension(state.clone()));
    let init = host
        .initialize_with(dir.to_str().unwrap(), &["providerAuth"])
        .await;
    let declared = &init["providers"][0];
    assert_eq!(declared["name"], "anthropic-2");
    assert_eq!(
        declared["config"]["models"],
        json!([]),
        "an account adds no models"
    );
    assert_eq!(declared["config"]["oauth"]["builtin"], "anthropic");

    let model = ctx(Some(("anthropic", SONNET)));
    let auth = json!({ "name": "before_provider_auth", "event": { "type": "before_provider_auth", "provider": "anthropic", "model": { "provider": "anthropic", "id": SONNET, "api": "anthropic-messages" } }, "ctx": model });
    host.request(1, "event", auth.clone()).await;
    let (response, _) = host.serve_until(1, pi).await;
    assert_eq!(
        response["result"]["result"],
        Value::Null,
        "account 1 is pi's own login"
    );

    // A limit error on account 1 moves the retry to account 2.
    let message = json!({ "role": "assistant", "content": [], "provider": "anthropic", "model": SONNET, "stopReason": "error", "errorMessage": "HTTP 429: rate_limit_error" });
    host.request(2, "event", json!({ "name": "message_end", "event": { "type": "message_end", "message": message }, "ctx": model })).await;
    let (response, seen) = host.serve_until(2, pi).await;
    let error = response["result"]["result"]["message"]["errorMessage"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert_eq!(error, "HTTP 429: rate limited; retrying on Claude #2");
    assert!(
        seen.iter().all(|(method, _)| method != "model.set"),
        "the model stays the same"
    );
    host.request(3, "event", auth).await;
    let (response, _) = host.serve_until(3, pi).await;
    assert_eq!(
        response["result"]["result"],
        json!({ "authProvider": "anthropic-2" })
    );
    host.finish().await.unwrap();
}

fn iso_in(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(now_ms() + ms)
        .unwrap()
        .to_rfc3339()
}

async fn accounts_host(name: &str, config: Value, mock: &MockHttp) -> (TestHost, Arc<State>) {
    let dir = temp_dir(name);
    std::fs::create_dir_all(dir.join("ext/subscription-router")).unwrap();
    std::fs::write(dir.join("ext/subscription-router.json"), config.to_string()).unwrap();
    let urls = Urls {
        codex: format!("{}/wham/usage", mock.url),
        anthropic: format!("{}/api/oauth/usage", mock.url),
    };
    let state = Arc::new(State::with_urls(&dir, urls));
    let mut host = TestHost::start(pi_ext_subscription_router::extension(state.clone()));
    host.initialize_with(dir.to_str().unwrap(), &["providerAuth"])
        .await;
    (host, state)
}

fn with_token(method: &str, params: &Value) -> Value {
    match method {
        "auth.getProviderAuth" => {
            json!({ "apiKey": common::codex_jwt("acct-1"), "headers": {}, "source": "oauth" })
        }
        _ => pi(method, params),
    }
}

fn limit_error(provider: &str, id: &str) -> Value {
    let message = json!({ "role": "assistant", "content": [], "provider": provider, "model": id, "stopReason": "error", "errorMessage": "429 You've hit your usage limit" });
    json!({ "name": "message_end", "event": { "type": "message_end", "message": message }, "ctx": ctx(Some((provider, id))) })
}

#[tokio::test]
async fn a_full_codex_account_spends_a_banked_reset_when_allowed() {
    use common::http::Reply;
    let full = json!({ "plan_type": "pro", "rate_limit": {
        "primary_window": { "used_percent": 100, "limit_window_seconds": 18000, "reset_after_seconds": 10800 },
        "secondary_window": { "used_percent": 40, "limit_window_seconds": 604800, "reset_after_seconds": 300000 } } });
    let fresh = json!({ "plan_type": "pro", "rate_limit": {
        "primary_window": { "used_percent": 0, "limit_window_seconds": 18000, "reset_after_seconds": 18000 },
        "secondary_window": { "used_percent": 0, "limit_window_seconds": 604800, "reset_after_seconds": 604800 } } });
    let mock = MockHttp::start(vec![
        Reply::json(200, full),
        Reply::json(200, json!({ "available_count": 1, "credits": [{ "id": "c1", "reset_type": "full", "status": "available" }] })),
        Reply::json(200, json!({ "code": "reset", "windows_reset": 2 })),
        Reply::json(200, fresh),
    ])
    .await;
    let config =
        json!({ "accounts": { "openai-codex": { "strategy": "fill", "useResets": "when-full" } } });
    let (mut host, _state) = accounts_host("resets", config, &mock).await;
    host.request(1, "event", limit_error("openai-codex", "gpt-5.5"))
        .await;
    let (response, seen) = host.serve_until(1, with_token).await;
    assert_eq!(
        response["result"]["result"]["message"]["errorMessage"],
        "HTTP 429: rate limited; retrying Codex #1 after a usage reset"
    );
    let notice = seen.iter().find(|(m, _)| m == "ui.notify").expect("notice");
    assert_eq!(
        notice.1["message"],
        "subscription-router: used a usage reset on Codex #1 (0 left)."
    );
    let requests = mock.requests();
    let consume = requests
        .iter()
        .find(|r| r.path.ends_with("/consume"))
        .expect("consume");
    assert_eq!(consume.method, "POST");
    assert_eq!(
        requests
            .iter()
            .find(|r| r.path.ends_with("/rate-limit-reset-credits"))
            .unwrap()
            .method,
        "GET"
    );
    let body: Value = serde_json::from_str(&consume.body).unwrap();
    assert_eq!(body["redeem_request_id"].as_str().map(str::len), Some(36));
    assert_eq!(
        consume
            .headers
            .get("chatgpt-account-id")
            .map(String::as_str),
        Some("acct-1")
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn every_account_full_falls_back_and_returns_when_one_has_room() {
    use common::http::Reply;
    let full = json!({ "five_hour": { "utilization": 100, "resets_at": iso_in(3 * HOUR) },
        "seven_day": { "utilization": 50, "resets_at": iso_in(72 * HOUR) } });
    let mock = MockHttp::start(vec![Reply::json(200, full)]).await;
    let config = json!({ "accounts": { "anthropic": { "strategy": "fill",
        "fallback": [{ "model": "openai-codex/gpt-5.5", "thinking": "high" }] } } });
    let (mut host, state) = accounts_host("fallback", config, &mock).await;
    let token = |method: &str, params: &Value| match method {
        "auth.getProviderAuth" => {
            json!({ "apiKey": "sk-ant-oat01-one", "headers": {}, "source": "oauth" })
        }
        _ => pi(method, params),
    };
    host.request(1, "event", limit_error("anthropic", SONNET))
        .await;
    let (response, seen) = host.serve_until(1, token).await;
    assert_eq!(
        response["result"]["result"]["message"]["errorMessage"],
        "HTTP 429: rate limited; falling back to openai-codex/gpt-5.5"
    );
    let set = seen
        .iter()
        .find(|(m, _)| m == "model.set")
        .expect("model.set");
    assert_eq!(
        set.1,
        json!({ "provider": "openai-codex", "id": "gpt-5.5" })
    );

    // Still full at the next prompt: stay on the fallback.
    let start = |id| json!({ "name": "before_agent_start", "event": { "type": "before_agent_start", "prompt": "go" }, "ctx": ctx(Some(("openai-codex", id))) });
    host.request(2, "event", start("gpt-5.5")).await;
    let (_, seen) = host.serve_until(2, token).await;
    assert!(seen.iter().all(|(m, _)| m != "model.set"));

    // The account frees up: back to Anthropic.
    lock(&state.picks).full_until.clear();
    state.usage().insert(usage(1, 5.0, 50.0, None));
    host.request(3, "event", start("gpt-5.5")).await;
    let (_, seen) = host.serve_until(3, token).await;
    let set = seen
        .iter()
        .find(|(m, _)| m == "model.set")
        .expect("model.set back");
    assert_eq!(set.1, json!({ "provider": "anthropic", "id": SONNET }));
    assert!(lock(&state.picks).fallback_from.is_none());
    host.finish().await.unwrap();
}

#[tokio::test]
async fn a_session_on_a_retired_account_model_moves_to_the_base_model_on_that_account() {
    let mock = MockHttp::start(vec![]).await;
    let dir = temp_dir("migrate");
    std::fs::create_dir_all(dir.join("ext/subscription-router")).unwrap();
    let accounts = json!({ "version": 1, "accounts": { "anthropic-2": { "type": "pi" } } });
    std::fs::write(
        dir.join("ext/subscription-router/accounts.json"),
        accounts.to_string(),
    )
    .unwrap();
    let urls = Urls {
        codex: format!("{}/wham/usage", mock.url),
        anthropic: format!("{}/api/oauth/usage", mock.url),
    };
    let state = Arc::new(State::with_urls(&dir, urls));
    let mut host = TestHost::start(pi_ext_subscription_router::extension(state.clone()));
    host.initialize_with(dir.to_str().unwrap(), &["providerAuth"])
        .await;
    let start = json!({ "name": "before_agent_start", "event": { "type": "before_agent_start", "prompt": "go" }, "ctx": ctx(Some(("anthropic-2", SONNET))) });
    host.request(1, "event", start).await;
    let (_, seen) = host.serve_until(1, pi).await;
    let set = seen
        .iter()
        .find(|(m, _)| m == "model.set")
        .expect("model.set");
    assert_eq!(set.1, json!({ "provider": "anthropic", "id": SONNET }));
    assert_eq!(lock(&state.picks).pinned.get(&Family::Anthropic), Some(&2));
    host.finish().await.unwrap();
}
