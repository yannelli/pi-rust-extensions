mod common;

use std::sync::atomic::Ordering;

use common::fake_cdp::FakeCdp;
use common::{Pi, text};
use pi_ext_chrome::{NOT_AUTHORIZED, TOOL_NAMES, extension};
use serde_json::json;

struct Setup {
    pi: Pi,
    fake: FakeCdp,
    cwd: tempfile::TempDir,
    _agent: tempfile::TempDir,
}

async fn setup() -> Setup {
    let fake = FakeCdp::start().await;
    let agent = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let config = json!({ "mode": "connect", "port": fake.http_port });
    std::fs::create_dir_all(agent.path().join("ext")).unwrap();
    std::fs::write(agent.path().join("ext/chrome.json"), config.to_string()).unwrap();
    let mut pi = Pi::start(extension(), cwd.path());
    let init = pi.initialize(agent.path()).await;
    let names: Vec<&str> = init["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, TOOL_NAMES);
    let reply = pi
        .event("session_start", json!({ "reason": "startup" }))
        .await;
    assert!(reply.get("error").is_none(), "{reply}");
    Setup {
        pi,
        fake,
        cwd,
        _agent: agent,
    }
}

#[tokio::test]
async fn locked_session_hides_tools_and_blocks_calls_without_network() {
    let Setup { mut pi, fake, .. } = setup().await;
    assert_eq!(pi.active, ["read", "bash"]);
    assert_eq!(pi.chrome_active(), 0);
    let result = pi
        .tool("chrome_navigate", json!({ "url": "https://example.com" }))
        .await;
    assert_eq!(result["isError"], true);
    assert!(text(&result).starts_with("Chrome is not authorized for this session."));
    assert_eq!(text(&result), NOT_AUTHORIZED);
    assert_eq!(fake.http_hits.load(Ordering::SeqCst), 0);
    assert!(fake.methods().is_empty());
    let reply = pi
        .event(
            "before_agent_start",
            json!({ "systemPrompt": "", "systemPromptOptions": {} }),
        )
        .await;
    assert_eq!(
        reply["result"]["patch"]["systemPromptOptions"]["sections"]["chrome-profile-bridge"],
        json!(null)
    );
    pi.finish().await;
}

#[tokio::test]
async fn declined_confirm_keeps_tools_locked() {
    let Setup { mut pi, fake, .. } = setup().await;
    pi.confirm = false;
    pi.command("authorize 15m").await;
    assert_eq!(pi.chrome_active(), 0);
    assert!(pi.seen_methods().contains(&"ui.confirm".to_owned()));
    assert_eq!(fake.http_hits.load(Ordering::SeqCst), 0);
    pi.finish().await;
}

#[tokio::test]
async fn authorized_session_drives_chrome_over_cdp() {
    let Setup {
        mut pi, fake, cwd, ..
    } = setup().await;
    pi.command("authorize 30m").await;
    assert_eq!(pi.chrome_active(), TOOL_NAMES.len());
    assert!(pi.active.contains(&"read".to_owned()));
    let entry = pi.branch.last().unwrap();
    assert_eq!(entry["customType"], "pi-chrome-tool-change");
    assert_eq!(
        (
            entry["data"]["action"].as_str(),
            entry["data"]["sessionId"].as_str()
        ),
        (Some("authorized"), Some("s1"))
    );
    let reply = pi
        .event(
            "before_agent_start",
            json!({ "systemPrompt": "", "systemPromptOptions": {} }),
        )
        .await;
    let primer =
        &reply["result"]["patch"]["systemPromptOptions"]["sections"]["chrome-profile-bridge"];
    assert!(primer.as_str().unwrap().contains("chrome_snapshot"));

    let result = pi
        .tool("chrome_navigate", json!({ "url": "https://example.com" }))
        .await;
    assert_eq!(
        text(&result),
        "Navigated to https://example.com/\nTitle: Example"
    );
    let navigate = &fake.calls_of("Page.navigate")[0];
    assert_eq!(
        (
            navigate["sessionId"].as_str(),
            navigate["params"]["url"].as_str()
        ),
        (Some("S-T1"), Some("https://example.com"))
    );
    assert_eq!(
        fake.calls_of("Target.createTarget")[0]["params"]["background"],
        true
    );

    let result = pi.tool("chrome_snapshot", json!({})).await;
    assert!(
        text(&result).contains(r#"[e1] button "Sign in" (10,20)"#),
        "{result}"
    );
    let result = pi.tool("chrome_click", json!({ "uid": "e1" })).await;
    assert_eq!(text(&result), "Clicked e1.");
    let mouse = fake.calls_of("Input.dispatchMouseEvent");
    let kinds: Vec<&str> = mouse
        .iter()
        .map(|m| m["params"]["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["mouseMoved", "mousePressed", "mouseReleased"]);
    assert_eq!(
        (
            mouse[1]["params"]["x"].as_f64(),
            mouse[1]["params"]["y"].as_f64()
        ),
        (Some(10.0), Some(20.0))
    );

    let result = pi
        .tool(
            "chrome_type",
            json!({ "uid": "e1", "text": "hello", "pressEnter": true }),
        )
        .await;
    assert!(
        text(&result).starts_with("Typed 5 character(s) into e1."),
        "{result}"
    );
    assert_eq!(
        fake.calls_of("Input.insertText")[0]["params"]["text"],
        "hello"
    );
    let keys = fake.calls_of("Input.dispatchKeyEvent");
    assert_eq!(
        (
            keys[0]["params"]["key"].as_str(),
            keys[0]["params"]["type"].as_str()
        ),
        (Some("Enter"), Some("keyDown"))
    );

    let result = pi
        .tool(
            "chrome_evaluate",
            json!({ "expression": "throw new Error('boom')" }),
        )
        .await;
    assert_eq!(
        (result["isError"].as_bool(), text(&result)),
        (Some(true), "JavaScript error: Error: boom".to_owned())
    );

    let result = pi
        .tool("chrome_screenshot", json!({ "path": "shot.png" }))
        .await;
    assert!(
        text(&result).starts_with("Saved Chrome screenshot to"),
        "{result}"
    );
    assert_eq!(
        std::fs::read(cwd.path().join("shot.png")).unwrap(),
        b"PNGDATA"
    );

    let result = pi.tool("chrome_tab", json!({ "action": "list" })).await;
    assert!(
        text(&result).contains("USER\tUser tab") && text(&result).contains("T1\t[session]"),
        "{result}"
    );
    let result = pi
        .tool(
            "chrome_tab",
            json!({ "action": "activate", "targetId": "USER" }),
        )
        .await;
    assert!(text(&result).contains("Background mode is on"));
    let result = pi
        .tool("chrome_cdp", json!({ "method": "Page.bringToFront" }))
        .await;
    assert_eq!(result["isError"], true);
    let result = pi
        .tool(
            "chrome_cdp",
            json!({ "method": "DOM.getDocument", "depth": 1 }),
        )
        .await;
    assert!(text(&result).contains("unknown top-level parameter(s): depth"));
    assert!(
        fake.calls_of("Target.activateTarget").is_empty()
            && fake.calls_of("Page.bringToFront").is_empty()
    );

    pi.command("revoke").await;
    assert_eq!(pi.chrome_active(), 0);
    assert_eq!(
        fake.calls_of("Target.closeTarget")[0]["params"]["targetId"],
        "T1"
    );
    assert!(
        fake.targets
            .lock()
            .unwrap()
            .iter()
            .any(|t| t["targetId"] == "USER")
    );
    assert_eq!(pi.branch.last().unwrap()["data"]["action"], "revoked");
    pi.finish().await;
}

#[tokio::test]
async fn remote_websocket_url_is_refused() {
    let Setup { mut pi, fake, .. } = setup().await;
    *fake.ws_url.lock().unwrap() = Some(format!(
        "ws://evil.example:{}/devtools/browser/fake-1",
        fake.http_port
    ));
    pi.command("authorize 30m").await;
    let result = pi
        .tool("chrome_navigate", json!({ "url": "https://example.com" }))
        .await;
    assert_eq!(result["isError"], true);
    assert!(
        text(&result).contains("refusing webSocketDebuggerUrl ws://evil.example"),
        "{result}"
    );
    assert_eq!(fake.http_hits.load(Ordering::SeqCst), 1);
    assert!(fake.methods().is_empty());
    pi.finish().await;
}

#[tokio::test]
async fn reload_restores_grant_of_same_session_only() {
    let Setup { mut pi, .. } = setup().await;
    pi.command("authorize indefinite").await;
    let branch = pi.branch.clone();
    pi.finish().await;

    let fake = FakeCdp::start().await;
    let (agent, cwd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    for (session, expected) in [("s1", TOOL_NAMES.len()), ("s2", 0)] {
        let mut pi = Pi::start(extension(), cwd.path());
        pi.session_id = session.to_owned();
        pi.branch = branch.clone();
        pi.initialize(agent.path()).await;
        pi.event("session_start", json!({ "reason": "reload" }))
            .await;
        assert_eq!(pi.chrome_active(), expected, "session {session}");
        pi.finish().await;
    }
    assert_eq!(fake.http_hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn completions_offer_subcommands_and_durations() {
    let Setup { mut pi, .. } = setup().await;
    let reply = pi
        .request(
            "command.complete",
            json!({ "name": "chrome", "prefix": "au", "ctx": pi.ctx() }),
        )
        .await;
    assert_eq!(reply["result"][0]["value"], "authorize");
    let reply = pi
        .request(
            "command.complete",
            json!({ "name": "chrome", "prefix": "authorize 3", "ctx": pi.ctx() }),
        )
        .await;
    assert_eq!(reply["result"][0]["value"], "authorize 30m");
    pi.finish().await;
}
