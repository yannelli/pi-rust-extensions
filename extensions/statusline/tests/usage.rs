//! The `usage` segment renders from a mocked `usage.get` reply and stays hidden on old hosts.

mod common;
mod support;

use common::TestHost;
use pi_ext_sdk::prelude::*;
use pi_ext_statusline::segments::usage;
use support::*;

fn fetcher() -> Extension {
    Extension::new("usage-test").command("fetch", "", |provider, ctx: CommandCtx| async move {
        if !usage::fetch(ctx.host(), &provider, false).await {
            bail!("no usage.get");
        }
        Ok(())
    })
}

fn on(provider: &str) -> StatusSnapshot {
    StatusSnapshot {
        model: Some(StatusModel {
            provider: provider.into(),
            id: "m".into(),
            name: "m".into(),
            reasoning: false,
        }),
        ..StatusSnapshot::default()
    }
}

fn render(segment: &str, snapshot: &StatusSnapshot) -> Vec<String> {
    let config = settings(json!({ "chip": "plain", "icons": "none", "segments": [segment] }));
    plain(&BuiltinStatusLine::from_settings(&config, 0).render(snapshot, 80))
}

#[tokio::test]
async fn usage_segment_renders_from_usage_get() {
    let mut host = TestHost::start(fetcher());
    host.initialize().await;
    let params = json!({ "name": "fetch", "args": "anthropic", "ctx": common::ctx() });
    host.request(1, "command.run", params).await;
    let (id, params) = host.expect_request("usage.get").await;
    assert_eq!(params, json!({ "provider": "anthropic" }));
    let reply = json!({ "providers": [{
        "provider": "anthropic", "plan": "max", "error": null, "fetchedAt": 1,
        "windows": [
            { "label": "5h", "usedPercent": 41.6, "resetsAt": null, "windowMinutes": 300 },
            { "label": "Weekly", "usedPercent": 75.0, "resetsAt": null, "windowMinutes": 10080 }
        ]
    }] });
    host.answer(&id, reply).await;
    assert_eq!(host.expect_response(1).await["result"], Value::Null);

    let snapshot = on("anthropic");
    assert_eq!(
        segment_text(Segment::Usage, &snapshot).as_deref(),
        Some("5h 42% · wk 75%")
    );
    assert_eq!(render("usage", &snapshot)[0].trim_end(), "5h 42% · wk 75%");
    assert_eq!(Segment::UsageReset.role(&snapshot), Role::ContextWarn);
    assert_eq!(segment_text(Segment::Usage, &on("openrouter")), None);
    assert!(
        render("usageReset", &on("openai-codex"))
            .concat()
            .trim()
            .is_empty()
    );

    let params = json!({ "name": "fetch", "args": "openai", "ctx": common::ctx() });
    host.request(2, "command.run", params).await;
    let (id, _) = host.expect_request("usage.get").await;
    let error = json!({ "code": -32601, "message": "unknown method: usage.get" });
    host.send(json!({ "jsonrpc": "2.0", "id": id, "error": error }))
        .await;
    assert!(host.expect_response(2).await.get("error").is_some());
    assert_eq!(segment_text(Segment::Usage, &on("openai")), None);
    host.finish().await.unwrap();
}
