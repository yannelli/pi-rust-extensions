//! Usage read through pi's `usage.get`: the snapshot it gives, the refresh flag, the router's own
//! backoff over pi's errors, and the fallback to the router's own read for older hosts only.

mod common;

use std::sync::Mutex;

use common::http::{MockHttp, Reply};
use common::now_ms;
use pi_ext_sdk::ProviderUsage;
use pi_ext_subscription_router::usage::{
    BoxFuture, CoreRead, GetOptions, Scope, Status, Tokens, Urls, UsageService,
};
use serde_json::{Value, json};

fn urls(mock: &MockHttp) -> Urls {
    Urls {
        codex: format!("{}/wham/usage", mock.url),
        anthropic: format!("{}/api/oauth/usage", mock.url),
    }
}

/// An `anthropic-2` entry as pi sends it, with the fields of hosts that support slots.
fn anthropic(provider: &str) -> Value {
    json!({ "provider": provider, "plan": "max", "error": null, "fetchedAt": 1_000_000, "windows": [
        { "label": "5h", "usedPercent": 42, "resetsAt": "2026-10-30T17:00:00Z", "windowMinutes": 300, "scope": "provider", "id": "five_hour", "resetsAtMs": 1_793_379_600_123u64 },
        { "label": "Weekly Opus", "usedPercent": 100, "resetsAt": null, "windowMinutes": 10080, "scope": "model", "families": ["opus"], "id": "seven_day_opus" },
        { "label": "Weekly Haiku", "usedPercent": 3, "resetsAt": null, "windowMinutes": 10080, "scope": "model", "families": ["haiku"], "id": "weekly_scoped" }
    ] })
}

/// `usage.get` answers from a script; records each refresh flag.
struct Core {
    answer: Value,
    refreshes: Mutex<Vec<bool>>,
}

impl Core {
    fn new(answer: Value) -> Self {
        Self {
            answer,
            refreshes: Mutex::new(Vec::new()),
        }
    }
}

impl Tokens for Core {
    fn token<'a>(&'a self, _provider: &'a str) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { Some("router-token".to_owned()) })
    }

    fn core_usage<'a>(&'a self, provider: &'a str, refresh: bool) -> BoxFuture<'a, CoreRead> {
        self.refreshes.lock().unwrap().push(refresh);
        let usage: ProviderUsage = serde_json::from_value(self.answer.clone()).unwrap();
        Box::pin(async move {
            match pi_ext_subscription_router::usage::from_older_host(&usage) {
                true => CoreRead::Unsupported,
                false => CoreRead::Usage(ProviderUsage {
                    provider: provider.into(),
                    ..usage
                }),
            }
        })
    }
}

#[tokio::test]
async fn a_pi_read_keeps_model_windows_tier_and_exact_resets_without_http() {
    let mock = MockHttp::start(vec![]).await;
    let service = UsageService::new(urls(&mock), None);
    let core = Core::new(anthropic("anthropic-2"));
    let snapshot = service
        .get("anthropic-2", &core, GetOptions::default())
        .await;
    assert_eq!(
        (
            snapshot.status,
            snapshot.tier.as_deref(),
            snapshot.retrieved_at
        ),
        (Status::Available, Some("max"), 1_000_000)
    );
    let names: Vec<&str> = snapshot.windows.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(
        names,
        ["five_hour", "seven_day_opus"],
        "the router keeps the windows it always read"
    );
    assert_eq!(
        (
            snapshot.windows[0].reset_at,
            snapshot.windows[0].duration_ms
        ),
        (Some(1_793_379_600_123), Some(18_000_000))
    );
    let opus = &snapshot.windows[1];
    assert_eq!(
        (opus.scope, opus.families.as_deref(), opus.used_percent),
        (Scope::Model, Some(&["opus".to_owned()][..]), 100.0)
    );
    assert!(mock.requests().is_empty());
    service
        .get(
            "anthropic-2",
            &core,
            GetOptions {
                fresh: true,
                not_before: Some(now_ms()),
                ..GetOptions::default()
            },
        )
        .await;
    assert_eq!(
        *core.refreshes.lock().unwrap(),
        [false, true],
        "a fresh read did not ask pi to refresh"
    );
}

#[tokio::test]
async fn codex_reads_map_durations_credits_and_skip_extra_limits() {
    let mock = MockHttp::start(vec![]).await;
    let service = UsageService::new(urls(&mock), None);
    let answer = json!({ "provider": "openai-codex-2", "plan": "pro", "fetchedAt": 5, "credits": { "available": true, "unlimited": false, "balance": "3" },
        "windows": [
            { "label": "5h", "usedPercent": 10, "windowMinutes": 300, "scope": "provider", "id": "primary_window", "resetsAtMs": 900 },
            { "label": "Secondary", "usedPercent": 20, "windowMinutes": null, "scope": "provider", "id": "secondary_window" },
            { "label": "Spark 5h", "usedPercent": 99, "windowMinutes": 300, "scope": "provider", "id": "Spark/primary_window" }
        ] });
    let snapshot = service
        .get("openai-codex-2", &Core::new(answer), GetOptions::default())
        .await;
    let windows: Vec<(&str, Option<i64>, Option<&str>)> = snapshot
        .windows
        .iter()
        .map(|w| (w.name.as_str(), w.duration_ms, w.span_source.as_deref()))
        .collect();
    assert_eq!(
        windows,
        [
            ("5h", Some(18_000_000), Some("reported")),
            ("7d", Some(604_800_000), Some("assumed"))
        ]
    );
    let credits = snapshot.credits.expect("credits");
    assert_eq!(
        (
            credits.available,
            credits.balance.as_deref(),
            snapshot.tier.as_deref()
        ),
        (true, Some("3"), Some("pro"))
    );
}

#[tokio::test]
async fn pi_failures_back_off_with_their_retry_after_and_send_no_http() {
    let mock = MockHttp::start(vec![]).await;
    let service = UsageService::new(urls(&mock), None);
    let limited = json!({ "provider": "anthropic-2", "error": "rate limited", "errorKind": "rateLimited", "retryAfterMs": 1_800_000, "windows": [] });
    let core = Core::new(limited);
    let first = service
        .get("anthropic-2", &core, GetOptions::default())
        .await;
    assert_eq!(first.status, Status::Error);
    let second = service
        .get("anthropic-2", &core, GetOptions::default())
        .await;
    let message = second.message.unwrap_or_default();
    assert!(message.starts_with("Backing off"), "{message}");
    let until = chrono::DateTime::parse_from_rfc3339(
        message.trim_end_matches('.').rsplit(' ').next().unwrap(),
    )
    .unwrap();
    assert!(
        until.timestamp_millis() - now_ms() > 1_700_000,
        "retry-after was not honored: {message}"
    );
    assert_eq!(core.refreshes.lock().unwrap().len(), 1);
    let login = json!({ "provider": "anthropic-2", "error": "Not logged in", "errorKind": "login", "windows": [] });
    let unavailable = UsageService::new(urls(&mock), None)
        .get("anthropic-2", &Core::new(login), GetOptions::default())
        .await;
    assert_eq!(unavailable.status, Status::Unavailable);
    assert!(
        mock.requests().is_empty(),
        "a pi error fell back to the router's own read"
    );
}

#[tokio::test]
async fn only_results_of_older_hosts_fall_back_to_the_router_read() {
    let body = json!({ "five_hour": { "utilization": 5, "resets_at": "2099-01-01T00:00:00Z" } });
    let mock = MockHttp::start(vec![Reply::json(200, body.clone()), Reply::json(200, body)]).await;
    let unscoped = json!({ "provider": "anthropic", "windows": [{ "label": "5h", "usedPercent": 1, "windowMinutes": 300 }] });
    let snapshot = UsageService::new(urls(&mock), None)
        .get("anthropic", &Core::new(unscoped), GetOptions::default())
        .await;
    assert_eq!(snapshot.windows[0].used_percent, 5.0);
    let unaliased = json!({ "provider": "anthropic-2", "error": "pi has no usage report for anthropic-2", "windows": [] });
    let snapshot = UsageService::new(urls(&mock), None)
        .get("anthropic-2", &Core::new(unaliased), GetOptions::default())
        .await;
    assert_eq!(snapshot.status, Status::Available);
    let requests = mock.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].headers["authorization"], "Bearer router-token");
}

#[tokio::test]
async fn exact_and_extreme_codex_lengths_and_times_map_without_overflow() {
    let mock = MockHttp::start(vec![]).await;
    let answer = json!({ "provider": "openai-codex-2", "fetchedAt": u64::MAX, "windows": [
        { "label": "5m", "usedPercent": 1, "windowMinutes": 5, "windowSeconds": 301, "scope": "provider", "id": "primary_window" },
        { "label": "Secondary", "usedPercent": 2, "windowMinutes": 5, "windowSeconds": u64::MAX, "scope": "provider", "id": "secondary_window", "resetsAtMs": i64::MAX }
    ] });
    let before = now_ms();
    let snapshot = UsageService::new(urls(&mock), None)
        .get("openai-codex-2", &Core::new(answer), GetOptions::default())
        .await;
    let windows: Vec<(&str, Option<i64>, Option<&str>)> = snapshot
        .windows
        .iter()
        .map(|w| (w.name.as_str(), w.duration_ms, w.span_source.as_deref()))
        .collect();
    assert_eq!(
        windows,
        [
            ("301s", Some(301_000), Some("reported")),
            ("7d", Some(604_800_000), Some("assumed"))
        ]
    );
    assert_eq!(snapshot.windows[1].reset_at, Some(i64::MAX));
    assert!(
        snapshot.retrieved_at >= before,
        "an out-of-range fetchedAt was kept"
    );
}
