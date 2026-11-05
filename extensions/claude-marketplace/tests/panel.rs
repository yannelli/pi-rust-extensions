//! The `/claude-plugins` panel over the protocol: tabs, an install with progress in the panel,
//! and the inline toggle, against a local marketplace.

mod common;

use common::TestHost;
use common::fixture::Fixture;
use pi_ext_claude_marketplace::{Scope, extension, market};
use serde_json::{Value, json};

fn text(lines: &Value) -> String {
    let lines = lines.as_array().unwrap().iter().filter_map(Value::as_str);
    let text = pi_ext_sdk::style::strip(&lines.collect::<Vec<_>>().join("\n"));
    text.lines()
        .map(|l| {
            l.split(' ')
                .filter(|w| !w.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Answers host requests with `null` until `method` arrives.
async fn until(host: &mut TestHost, method: &str) -> (Option<Value>, Value) {
    loop {
        let message = host.recv().await;
        let name = message["method"].as_str().unwrap_or_default().to_owned();
        let id = message.get("id").cloned();
        if name == method {
            return (id, message["params"].clone());
        }
        if let Some(id) = id.filter(|_| !name.is_empty()) {
            host.answer(&id, Value::Null).await;
        }
    }
}

/// Sends a key, then returns the text of the first panel update whose text contains `want`.
async fn key_until(host: &mut TestHost, surface: &Value, data: &str, want: &str) -> String {
    host.notify("ui.key", json!({ "surface": surface, "data": data }))
        .await;
    loop {
        let lines = text(&until(host, "ui.update").await.1["lines"]);
        if lines.contains(want) {
            return lines;
        }
    }
}

#[tokio::test]
async fn the_panel_installs_with_progress_and_toggles_inline() {
    let fx = Fixture::new();
    market::add(&fx.env(true), fx.repo.to_str().unwrap(), Scope::User).unwrap();
    let mut host = TestHost::start(extension(fx.paths()));
    host.initialize_in(fx.project.to_str().unwrap(), fx.agent.to_str().unwrap())
        .await;
    let ctx = json!({ "cwd": fx.project, "hasUI": true, "flags": {}, "isProjectTrusted": true });
    host.request(
        1,
        "command.run",
        json!({ "name": "claude-plugins", "args": "", "ctx": ctx }),
    )
    .await;
    let (panel, opened) = until(&mut host, "ui.custom").await;
    let (surface, lines) = (opened["surface"].clone(), text(&opened["lines"]));
    assert!(
        lines.contains("Claude plugins 0 installed · 1 marketplace ·"),
        "{lines}"
    );
    assert!(
        lines.contains("[Installed (0)] Discover (4) Marketplaces (1)"),
        "{lines}"
    );
    assert!(lines.contains("No plugins installed"), "{lines}");

    let lines = key_until(&mut host, &surface, "\t", "[Discover (4)]").await;
    assert!(lines.contains("→ + fmt@acme"), "{lines}");
    let lines = key_until(&mut host, &surface, "\r", "Actions").await;
    assert!(
        lines.contains("Install for every project") && lines.contains("Install for this project"),
        "{lines}"
    );
    // Install runs on a blocking thread; the panel shows it, then the result and the new counts.
    let lines = key_until(&mut host, &surface, "\r", "Installed (1)").await;
    assert!(lines.contains("Installed fmt@acme"), "{lines}");

    let lines = key_until(&mut host, &surface, "\u{1b}[Z", "[Installed (1)]").await;
    assert!(lines.contains("● fmt@acme"), "{lines}");
    let lines = key_until(&mut host, &surface, "\u{1b}[C", "off user").await;
    assert!(lines.contains("○ fmt@acme"), "{lines}");

    host.notify("ui.key", json!({ "surface": surface, "data": "\u{1b}" }))
        .await;
    until(&mut host, "ui.close").await;
    host.answer(&panel.unwrap(), Value::Null).await;
    assert_eq!(host.expect_response(1).await["result"], Value::Null);
}
