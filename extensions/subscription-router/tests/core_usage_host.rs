//! `/router refresh` against hosts with and without slot usage in `usage.get`: pi-owned slots are
//! read by pi, slots with the router's own token and older hosts are read here.

mod common;

use common::http::{MockHttp, Reply};
use common::{TestHost, ctx, now_ms, temp_dir};
use pi_ext_subscription_router::usage::Urls;
use serde_json::{Value, json};

fn urls(mock: &MockHttp) -> Urls {
    Urls {
        codex: format!("{}/wham/usage", mock.url),
        anthropic: format!("{}/api/oauth/usage", mock.url),
    }
}

fn anthropic(provider: &str) -> Value {
    json!({ "provider": provider, "plan": "max", "fetchedAt": 1_000_000, "windows": [
        { "label": "5h", "usedPercent": 42, "windowMinutes": 300, "scope": "provider", "id": "five_hour" }
    ] })
}

fn agent_dir(name: &str) -> std::path::PathBuf {
    let dir = temp_dir(name);
    std::fs::create_dir_all(dir.join("ext/subscription-router")).unwrap();
    let accounts = json!({ "version": 1, "accounts": {
        "anthropic-2": { "type": "pi" },
        "anthropic-3": { "type": "oauth", "access": "legacy-token", "refresh": "r", "expires": now_ms() + 3_600_000 } } });
    std::fs::write(
        dir.join("ext/subscription-router/accounts.json"),
        accounts.to_string(),
    )
    .unwrap();
    dir
}

/// `/router refresh` on a host: `usage` answers `usage.get` per provider.
async fn refresh(
    name: &str,
    usage: impl Fn(&str) -> Value,
) -> (Vec<(String, Value)>, Vec<common::http::Recorded>) {
    let body = json!({ "five_hour": { "utilization": 9, "resets_at": "2099-01-01T00:00:00Z" } });
    let mock = MockHttp::start(vec![
        Reply::json(200, body.clone()),
        Reply::json(200, body.clone()),
        Reply::json(200, body),
    ])
    .await;
    let dir = agent_dir(name);
    let state = std::sync::Arc::new(pi_ext_subscription_router::state::State::with_urls(
        &dir,
        urls(&mock),
    ));
    let mut host = TestHost::start(pi_ext_subscription_router::extension(state));
    host.initialize(dir.to_str().unwrap()).await;
    host.request(
        1,
        "command.run",
        json!({ "name": "router", "args": "refresh", "ctx": ctx(None) }),
    )
    .await;
    let (_, seen) = host
        .serve_until(1, |method, params| match method {
            "usage.get" => usage(params["provider"].as_str().unwrap_or_default()),
            "auth.getProviderAuth" => {
                json!({ "apiKey": "pi-token", "headers": {}, "source": "oauth" })
            }
            _ => Value::Null,
        })
        .await;
    host.finish().await.unwrap();
    (seen, mock.requests())
}

#[tokio::test]
async fn a_host_with_slot_usage_serves_pi_slots_and_legacy_tokens_stay_here() {
    let (seen, requests) = refresh(
        "core-usage-new",
        |provider| json!({ "providers": [anthropic(provider)] }),
    )
    .await;
    let asked: Vec<&Value> = seen
        .iter()
        .filter(|(m, _)| m == "usage.get")
        .map(|(_, p)| p)
        .collect();
    assert!(
        asked.contains(&&json!({ "provider": "anthropic-2", "refresh": true })),
        "{asked:?}"
    );
    assert!(
        !asked.iter().any(|p| p["provider"] == "anthropic-3"),
        "a legacy slot went to pi: {asked:?}"
    );
    let auths: Vec<&Value> = seen
        .iter()
        .filter(|(m, _)| m == "auth.getProviderAuth")
        .map(|(_, p)| p)
        .collect();
    assert!(
        auths.iter().all(|p| p["provider"] != "anthropic-2"),
        "{auths:?}"
    );
    let legacy: Vec<&str> = requests
        .iter()
        .map(|r| r.headers["authorization"].as_str())
        .collect();
    assert_eq!(
        legacy,
        ["Bearer legacy-token"],
        "only the legacy slot read the endpoint"
    );
}

#[tokio::test]
async fn a_host_without_usage_get_reads_every_slot_here() {
    let missing =
        |_: &str| json!({ "$error": { "code": -32601, "message": "Unknown method usage.get" } });
    let (_, requests) = refresh("core-usage-old", missing).await;
    let tokens: Vec<&str> = requests
        .iter()
        .map(|r| r.headers["authorization"].as_str())
        .collect();
    assert!(
        tokens.contains(&"Bearer legacy-token") && tokens.contains(&"Bearer pi-token"),
        "{tokens:?}"
    );
}
