//! `Host::usage` and `Host::usage_with`: `usage.get` params, result decoding, and old hosts.

use pi_ext_sdk::test_util::{TestHost, ctx};
use pi_ext_sdk::{ProviderUsage, UsageWindow, prelude::*};

fn usage_extension() -> Extension {
    Extension::new("usage")
        .command("all", "", |_, ctx: CommandCtx| async move {
            let usage = ctx.host().usage(None).await?;
            ctx.host().notify_info(serde_json::to_string(&usage)?).await
        })
        .command("one", "", |_, ctx: CommandCtx| async move {
            let usage = ctx.host().usage_with(Some("anthropic"), true).await?;
            ctx.host().notify_info(serde_json::to_string(&usage)?).await
        })
}

async fn run(host: &mut TestHost, id: u64, name: &str) {
    let params = json!({ "name": name, "args": "", "ctx": ctx() });
    host.request(id, "command.run", params).await;
}

async fn notified(host: &mut TestHost) -> Vec<ProviderUsage> {
    let (id, params) = host.expect_request("ui.notify").await;
    host.answer(&id, Value::Null).await;
    serde_json::from_str(params["message"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn usage_decodes_windows_and_tolerates_nulls() {
    let mut host = TestHost::start(usage_extension());
    host.initialize().await;
    run(&mut host, 4, "all").await;
    let (id, params) = host.expect_request("usage.get").await;
    assert_eq!(params, json!({}));
    let result = json!({ "providers": [
        { "provider": "anthropic", "plan": "max", "fetchedAt": 1_793_379_600_000u64, "error": null,
          "windows": [
            { "label": "5h", "usedPercent": 42.5, "resetsAt": "2026-10-30T17:00:00Z", "windowMinutes": 300 },
            { "label": null, "usedPercent": null, "resetsAt": null, "windowMinutes": null }
          ] },
        { "provider": "cursor", "plan": null, "windows": null, "error": "not logged in", "fetchedAt": null }
    ] });
    host.answer(&id, result).await;
    let usage = notified(&mut host).await;
    assert_eq!(host.expect_response(4).await["result"], Value::Null);
    let anthropic = ProviderUsage {
        provider: "anthropic".into(),
        plan: Some("max".into()),
        windows: vec![
            UsageWindow {
                label: "5h".into(),
                used_percent: 42.5,
                resets_at: Some("2026-10-30T17:00:00Z".into()),
                window_minutes: Some(300),
            },
            UsageWindow::default(),
        ],
        error: None,
        fetched_at: 1_793_379_600_000,
    };
    let cursor = ProviderUsage {
        provider: "cursor".into(),
        error: Some("not logged in".into()),
        ..ProviderUsage::default()
    };
    assert_eq!(usage, vec![anthropic, cursor]);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn usage_with_sends_provider_and_refresh() {
    let mut host = TestHost::start(usage_extension());
    host.initialize().await;
    run(&mut host, 4, "one").await;
    let (id, params) = host.expect_request("usage.get").await;
    assert_eq!(params, json!({ "provider": "anthropic", "refresh": true }));
    host.answer(&id, json!({ "providers": [{ "provider": "anthropic" }] }))
        .await;
    let usage = notified(&mut host).await;
    assert_eq!(usage[0].provider, "anthropic");
    assert!(usage[0].windows.is_empty() && usage[0].error.is_none());
    assert_eq!(host.expect_response(4).await["result"], Value::Null);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn usage_on_an_old_host_is_method_not_found() {
    let mut host = TestHost::start(usage_extension());
    host.initialize().await;
    run(&mut host, 4, "all").await;
    let (id, _) = host.expect_request("usage.get").await;
    let error = json!({ "code": -32601, "message": "unknown method: usage.get" });
    host.send(json!({ "jsonrpc": "2.0", "id": id, "error": error }))
        .await;
    let response = host.expect_response(4).await;
    assert_eq!(response["error"]["code"], -32601, "{response}");
    host.finish().await.unwrap();
}
