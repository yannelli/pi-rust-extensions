mod common;

use common::{Fake, ctx, skill};
use pi_ext_deferred::extension;
use pi_ext_sdk::test_util::TestHost;
use serde_json::{Value, json};

fn text(lines: &Value) -> String {
    let lines = lines.as_array().unwrap().iter().filter_map(Value::as_str);
    let text = pi_ext_sdk::style::strip(&lines.collect::<Vec<_>>().join("\n"));
    // Column padding varies with the width; the checks compare words.
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

/// Answers the extension's requests with `fake` until `method` arrives as a request or a notification.
async fn until(host: &mut TestHost, fake: &Fake, method: &str) -> (Option<Value>, Value) {
    loop {
        let message = host.recv().await;
        let name = message["method"].as_str().unwrap_or_default().to_owned();
        let id = message.get("id").cloned();
        if name == method {
            return (id, message["params"].clone());
        }
        if let Some(id) = id.filter(|_| !name.is_empty()) {
            let result = fake.answer(&name, &message["params"]);
            host.answer(&id, result).await;
        }
    }
}

async fn key(host: &mut TestHost, fake: &Fake, surface: &Value, data: &str) -> String {
    host.notify("ui.key", json!({ "surface": surface, "data": data }))
        .await;
    text(&until(host, fake, "ui.update").await.1["lines"])
}

#[tokio::test]
async fn panel_lists_everything_and_arrows_write_a_rule() {
    let dir = tempfile::tempdir().unwrap();
    let agent = dir.path().display().to_string();
    let mut fake = Fake::new(json!({ "tools": { "mcp__*": "names" } }), &["read", "bash"]);
    fake.options = json!({ "skills": [skill("/s", "pdf")], "contextFiles": [{ "path": "/w/AGENTS.md", "content": "# Rules" }], "sections": { "mcp_servers": "gh" } });
    let mut host = TestHost::start(extension()).agent_dir(agent.clone());
    host.initialize().await;
    let start = json!({ "type": "session_start", "reason": "startup" });
    let params = json!({ "name": "session_start", "event": start, "ctx": ctx("/work") });
    host.drive(1, "event", params, |m, p| fake.answer(m, p))
        .await;

    let params = json!({ "name": "deferred", "args": "", "ctx": ctx("/work") });
    host.request(2, "command.run", params).await;
    let (panel, opened) = until(&mut host, &fake, "ui.custom").await;
    let (surface, lines) = (opened["surface"].clone(), text(&opened["lines"]));
    assert!(lines.contains("Deferred context"), "{lines}");
    assert!(
        lines.contains("[Tools (4)] Skills (1) Context (1) Sections (1) Rules (1)"),
        "{lines}"
    );
    assert!(lines.contains("→ ● read on"), "{lines}");
    assert!(
        lines.contains("◐ mcp__gh__issue names ~") && lines.contains("tok rule mcp__*"),
        "{lines}"
    );

    let lines = key(&mut host, &fake, &surface, "\u{1b}[C").await;
    assert!(lines.contains("⋯ Saving the rule for read"), "{lines}");
    let lines = text(&until(&mut host, &fake, "ui.update").await.1["lines"]);
    assert!(lines.contains("Set tools `read` to names"), "{lines}");
    let set = fake.calls_of("settings.set");
    assert_eq!(
        set,
        [
            json!({ "namespace": "deferred", "value": { "tools": { "read": "names" } }, "scope": "global" })
        ]
    );

    let lines = key(&mut host, &fake, &surface, "\t").await;
    assert!(
        lines.contains("[Skills (1)]") && lines.contains("pdf"),
        "{lines}"
    );
    let lines = key(&mut host, &fake, &surface, "zz").await;
    assert!(
        lines.contains("No matches") && lines.contains("esc clear search"),
        "{lines}"
    );
    key(&mut host, &fake, &surface, "\u{1b}").await;
    host.notify("ui.key", json!({ "surface": surface, "data": "\u{1b}" }))
        .await;
    until(&mut host, &fake, "ui.close").await;
    host.answer(&panel.unwrap(), Value::Null).await;
    assert_eq!(host.expect_response(2).await["result"], Value::Null);
}

#[tokio::test]
async fn without_a_ui_the_command_prints_the_status() {
    let fake = Fake::new(json!({}), &[]);
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let mut no_ui = ctx("/work");
    no_ui["hasUI"] = json!(false);
    let params = json!({ "name": "deferred", "args": "", "ctx": no_ui });
    host.drive(1, "command.run", params, |m, p| fake.answer(m, p))
        .await;
    let note = fake.calls_of("ui.notify").pop().unwrap();
    assert!(
        note["message"]
            .as_str()
            .unwrap()
            .starts_with("deferred config:"),
        "{note}"
    );
}
