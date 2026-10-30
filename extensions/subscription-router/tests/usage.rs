mod common;

use common::http::{MockHttp, Reply};
use common::{codex_jwt, now_ms};
use pi_ext_subscription_router::config::ModelRef;
use pi_ext_subscription_router::usage::{
    BoxFuture, GetOptions, Scope, Status, Tokens, Urls, UsageService, codex_account_id,
    decision_for, format_for_prompt, parse_anthropic, parse_codex, retry_after_ms,
};
use serde_json::{Value, json};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

struct Fixed(Option<String>);

impl Tokens for Fixed {
    fn token<'a>(&'a self, _provider: &'a str) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move { self.0.clone() })
    }
}

#[test]
fn parses_codex_fixture() {
    let parsed = parse_codex(&fixture("codex_usage.json"), 1_000).unwrap();
    assert_eq!(parsed.tier.as_deref(), Some("pro"));
    let names: Vec<&str> = parsed.windows.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, ["5h", "7d"]);
    assert_eq!(parsed.windows[0].used_percent, 42.5);
    assert_eq!(parsed.windows[0].reset_at, Some(1_000 + 3_600_000));
    assert_eq!(parsed.windows[1].reset_at, Some(4_102_444_800_000));
    assert_eq!(parsed.windows[1].duration_ms, Some(604_800_000));
    let credits = parsed.credits.unwrap();
    assert!(credits.available && !credits.unlimited);
    assert!(parse_codex(&json!({ "rate_limit": {} }), 0).is_none());
}

#[test]
fn parses_anthropic_fixture_with_family_windows() {
    let parsed = parse_anthropic(&fixture("anthropic_usage.json")).unwrap();
    let names: Vec<&str> = parsed.windows.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, ["five_hour", "seven_day", "seven_day_opus"]);
    let opus = &parsed.windows[2];
    assert_eq!(opus.scope, Scope::Model);
    assert_eq!(opus.families.as_deref(), Some(&["opus".to_owned()][..]));
    assert_eq!(opus.used_percent, 100.0);
    assert_eq!(parsed.windows[0].reset_at, Some(4_102_462_800_000));
}

fn snapshot(provider: &str, payload: &Value) -> pi_ext_subscription_router::usage::UsageSnapshot {
    let parsed = parse_anthropic(payload).unwrap();
    pi_ext_subscription_router::usage::UsageSnapshot {
        provider: provider.into(),
        status: Status::Available,
        windows: parsed.windows,
        credits: None,
        tier: None,
        retrieved_at: now_ms(),
        expires_at: now_ms() + 300_000,
        message: None,
    }
}

#[test]
fn decisions_follow_scope_and_families() {
    let mut payload = fixture("anthropic_usage.json");
    payload["five_hour"]["utilization"] = json!(10);
    let snap = snapshot("anthropic", &payload);
    let opus = ModelRef::new("anthropic", "claude-opus-4-1");
    let sonnet = ModelRef::new("anthropic", "claude-sonnet-5");
    let magnopus = ModelRef::new("anthropic", "claude-magnopus");
    let opus_decision = decision_for(&opus, Some(&snap), now_ms());
    assert!(opus_decision.block);
    assert_eq!(opus_decision.scope, Some(Scope::Model));
    assert!(!decision_for(&sonnet, Some(&snap), now_ms()).block);
    assert!(!decision_for(&magnopus, Some(&snap), now_ms()).block);
    let full = snapshot("anthropic", &fixture("anthropic_usage.json"));
    assert!(decision_for(&sonnet, Some(&full), now_ms()).block);
    assert!(!decision_for(&sonnet, None, now_ms()).block);
}

#[test]
fn prompt_buckets_are_coarse() {
    let snap = snapshot("anthropic", &fixture("anthropic_usage.json"));
    let text = format_for_prompt(Some(&snap));
    assert_eq!(
        text,
        "five_hour exhausted until 2100-01-01T05:00Z; seven_day 25-50%; seven_day_opus exhausted, reset unknown"
    );
    assert_eq!(format_for_prompt(None), "usage unavailable");
}

#[test]
fn reads_jwt_account_and_retry_after() {
    assert_eq!(
        codex_account_id(&codex_jwt("acct-1")).as_deref(),
        Some("acct-1")
    );
    assert_eq!(codex_account_id("not-a-jwt"), None);
    assert_eq!(retry_after_ms("120", 0), Some(120_000));
    assert_eq!(
        retry_after_ms("Thu, 01 Jan 1970 00:01:00 GMT", 0),
        Some(60_000)
    );
    assert_eq!(retry_after_ms("0", 0), None);
}

fn urls(mock: &MockHttp) -> Urls {
    Urls {
        codex: format!("{}/wham/usage", mock.url),
        anthropic: format!("{}/api/oauth/usage", mock.url),
    }
}

#[tokio::test]
async fn codex_read_sends_account_header_and_caches() {
    let mock = MockHttp::start(vec![Reply::json(200, fixture("codex_usage.json"))]).await;
    let dir = common::temp_dir("usage-cache");
    let service = UsageService::new(urls(&mock), Some(dir.join("usage.json")));
    let tokens = Fixed(Some(codex_jwt("acct-9")));
    let first = service
        .get("openai-codex-2", &tokens, GetOptions::default())
        .await;
    assert_eq!(first.status, Status::Available, "{first:?}");
    let second = service
        .get("openai-codex-2", &tokens, GetOptions::default())
        .await;
    assert_eq!(second.retrieved_at, first.retrieved_at);
    let requests = mock.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/wham/usage");
    assert_eq!(requests[0].headers["chatgpt-account-id"], "acct-9");
    assert!(requests[0].headers["authorization"].starts_with("Bearer "));
    let stored: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("usage.json")).unwrap()).unwrap();
    assert_eq!(stored["snapshots"]["openai-codex-2"]["status"], "available");
    let reloaded = UsageService::new(urls(&mock), Some(dir.join("usage.json")));
    assert!(reloaded.peek("openai-codex-2").is_some());
}

#[tokio::test]
async fn failures_back_off_and_honor_retry_after() {
    let mock = MockHttp::start(vec![
        Reply::json(429, json!({})).header("Retry-After", "600"),
    ])
    .await;
    let service = UsageService::new(urls(&mock), None);
    let tokens = Fixed(Some("sk-ant-oat01-token".into()));
    let failed = service
        .get("anthropic", &tokens, GetOptions::default())
        .await;
    assert_eq!(failed.status, Status::Error);
    assert_eq!(
        failed.message.as_deref(),
        Some("Usage request failed (429).")
    );
    let again = service
        .get(
            "anthropic",
            &tokens,
            GetOptions {
                fresh: true,
                ..Default::default()
            },
        )
        .await;
    assert!(again.message.unwrap().starts_with("Backing off"));
    assert_eq!(mock.requests().len(), 1);
    assert_eq!(
        mock.requests()[0].headers["anthropic-beta"],
        "oauth-2025-04-20"
    );
}

#[tokio::test]
async fn api_keys_and_unknown_providers_are_not_read() {
    let mock = MockHttp::start(vec![Reply::json(200, json!({}))]).await;
    let service = UsageService::new(urls(&mock), None);
    let key = Fixed(Some("sk-ant-api03-key".into()));
    let snap = service
        .get("anthropic-2", &key, GetOptions::default())
        .await;
    assert_eq!(snap.status, Status::Unavailable);
    let snap = service.get("mock", &key, GetOptions::default()).await;
    assert_eq!(snap.status, Status::Unavailable);
    let none = service
        .get("openai-codex", &Fixed(None), GetOptions::default())
        .await;
    assert_eq!(none.status, Status::Unavailable);
    assert!(mock.requests().is_empty());
}

#[tokio::test]
async fn manual_refresh_is_limited_to_once_a_minute() {
    let mock = MockHttp::start(vec![Reply::json(200, fixture("codex_usage.json"))]).await;
    let service = UsageService::new(urls(&mock), None);
    let tokens = Fixed(Some(codex_jwt("a")));
    let force = GetOptions {
        force: true,
        ..Default::default()
    };
    assert_eq!(
        service.get("openai-codex", &tokens, force).await.status,
        Status::Available
    );
    let limited = service.get("openai-codex", &tokens, force).await;
    assert_eq!(limited.status, Status::Stale);
    assert_eq!(mock.requests().len(), 1);
}
