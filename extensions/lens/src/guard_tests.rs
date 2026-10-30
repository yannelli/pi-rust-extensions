#![cfg(unix)]

use serde_json::json;

use super::Project;
use super::protocol::Host;
use crate::state::{Guard, small_text};

#[tokio::test]
async fn read_of_a_large_file_appends_a_small_entry() {
    let project = Project::new();
    project.write("agent/ext/lens.json", r#"{"autoDetect":false}"#);
    let text = "abcdefghij\n".repeat(150_000);
    project.write("big.py", &text);
    let mut host = Host::new(crate::extension());
    host.request("initialize", json!({"protocolVersion":1,"cwd":project.0,"agentDir":project.0.join("agent"),"hasUI":false})).await;
    host.event("session_start", json!({}), &project).await;
    let read = json!({"toolName":"read","input":{"path":"big.py"},"content":[{"type":"text","text":text}],"isError":false});
    host.event("tool_result", read, &project).await;
    let entry = host
        .calls
        .iter()
        .rfind(|call| call["method"] == "session.appendEntry")
        .expect("appendEntry call")
        .to_string();
    assert!(entry.len() < 1024, "{} bytes", entry.len());
    assert!(!entry.contains("abcdefghij"));
    let edit = json!({"toolName":"edit","input":{"path":"big.py","oldText":"abc"}});
    let reply = host.event("tool_call", edit, &project).await;
    assert!(reply.get("result").is_none(), "{reply}");
    host.finish().await;
}

#[test]
fn entries_with_full_text_still_load() {
    let project = Project::new();
    let path = project.write("a.py", "one\ntwo\n");
    let old =
        json!({"reads": {path.to_string_lossy(): {"text": "one\ntwo\n", "ranges": [[1, 2]]}}});
    let mut guard: Guard = serde_json::from_value(old).unwrap();
    assert!(
        guard
            .block_reason(&path, small_text(&path), "write", &json!({}))
            .is_none()
    );
    let saved = serde_json::to_string(&guard).unwrap();
    assert!(
        !saved.contains("one\\ntwo") && saved.contains("hash"),
        "{saved}"
    );
    let mut reloaded: Guard = serde_json::from_str(&saved).unwrap();
    assert!(
        reloaded
            .block_reason(&path, small_text(&path), "write", &json!({}))
            .is_none()
    );
    project.write("a.py", "one\nchanged\n");
    assert!(
        reloaded
            .block_reason(&path, small_text(&path), "write", &json!({}))
            .is_some()
    );
}
