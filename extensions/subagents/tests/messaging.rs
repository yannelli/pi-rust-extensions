//! `agent_message` and mailbox delivery at the protocol level.

#![cfg(unix)]

mod common;

use common::{TestHost, ctx, dirs, scripted_events};
use pi_ext_subagents::messaging::{Letter, mailbox_root, post};
use serde_json::{Value, json};

fn session_ctx(cwd: &std::path::Path) -> Value {
    let mut ctx = ctx(cwd);
    ctx["sessionId"] = json!("parent-session");
    ctx
}

fn send(to: &str, cwd: &std::path::Path) -> Value {
    let params = json!({ "to": to, "message": "status?" });
    json!({ "toolCallId": "m1", "name": "agent_message", "params": params, "ctx": session_ctx(cwd) })
}

#[tokio::test]
async fn letters_in_the_session_mailbox_wake_the_agent_and_are_removed() {
    let d = dirs(&scripted_events("unused"), 0);
    let mut host = TestHost::start(pi_ext_subagents::extension());
    host.initialize(&d.agent, &d.cwd).await;
    let start = json!({ "name": "session_start", "event": { "type": "session_start" }, "ctx": session_ctx(&d.cwd) });
    host.request(9, "event", start).await;
    let registered = host.until_method("register.tool").await;
    assert_eq!(registered["name"], "agent_message");

    host.request(1, "tool.execute", send("parent", &d.cwd))
        .await;
    let response = host.until_response(1).await;
    assert_eq!(response["result"]["isError"], true);
    host.request(2, "tool.execute", send("run-missing", &d.cwd))
        .await;
    let response = host.until_response(2).await;
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("No running agent run-missing."), "{text}");

    let root = mailbox_root(&d.agent);
    let letter = Letter {
        from: "run-7".into(),
        agent: "build".into(),
        text: "tests pass".into(),
        sent_at: 1,
    };
    let path = post(&root, "parent-session", &letter).unwrap();
    let message = host.until_method("session.sendMessage").await;
    assert_eq!(message["customType"], "agent-message");
    assert_eq!(message["deliverAs"], "steer");
    assert_eq!(message["triggerTurn"], true);
    assert!(message["content"].as_str().unwrap().contains("tests pass"));
    let wake = host.until_method("events.emit").await;
    assert_eq!(wake["channel"], "agent:wake");
    assert!(!path.exists());
    host.finish().await;
}
