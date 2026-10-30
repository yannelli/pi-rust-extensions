//! Races between a detached completion audit and pause, clear, Esc, and resubmission.

mod common;

use std::time::Duration;

use common::{Host, TestDir};
use serde_json::{Value, json};

const OBJECTIVE: &str = "Write the release notes for version 2";
const QUIET: Duration = Duration::from_millis(300);

fn verdict(text: &str) -> Value {
    json!({ "message": { "role": "assistant" }, "text": text })
}

async fn start(label: &str) -> (Host, TestDir, TestDir) {
    let work = TestDir::new(&format!("{label}-work"));
    let agent = TestDir::new(&format!("{label}-agent"));
    let mut host = Host::start(work.path(), agent.path()).await;
    create(&mut host, OBJECTIVE).await;
    (host, work, agent)
}

async fn create(host: &mut Host, objective: &str) {
    let (created, failed) = host
        .tool("create_goal", json!({ "objective": objective }))
        .await;
    assert!(!failed, "{created}");
}

fn complete() -> Value {
    json!({ "status": "complete", "completion_summary": "Notes written." })
}

/// Requests completion and returns once the auditor's model request is held.
async fn submit(host: &mut Host) {
    host.hold.push("llm.complete");
    let complete = complete();
    let (submitted, failed) = host.tool("update_goal", complete).await;
    assert!(
        !failed && submitted.starts_with("Completion submitted"),
        "{submitted}"
    );
    host.wait_for("llm.complete").await;
    let (summary, _) = host.tool("get_goal", json!({})).await;
    assert!(summary.contains("auditing"), "{summary}");
}

async fn command(host: &mut Host, name: &str) {
    let ctx = host.ctx.clone();
    let params = json!({ "name": name, "args": "", "ctx": ctx });
    host.request("command.run", params).await;
}

fn audit_messages(host: &Host) -> Vec<String> {
    let sent = host.called("session.sendMessage");
    let audits = sent
        .iter()
        .filter(|m| m["customType"] == "pi-goal-audit-event");
    audits
        .map(|m| m["content"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[tokio::test]
async fn pause_discards_a_late_approval() {
    let (mut host, _work, _agent) = start("pause").await;
    submit(&mut host).await;
    command(&mut host, "goal-pause").await;
    host.release("llm.complete", verdict("<approved/>")).await;
    host.drain(QUIET).await;
    let (summary, _) = host.tool("get_goal", json!({})).await;
    assert!(summary.contains(OBJECTIVE), "{summary}");
    assert!(summary.contains("paused"), "{summary}");
    assert!(
        audit_messages(&host).is_empty(),
        "{:?}",
        audit_messages(&host)
    );
}

#[tokio::test]
async fn esc_during_audit_pauses_and_discards_a_rejection() {
    let (mut host, _work, _agent) = start("esc").await;
    submit(&mut host).await;
    host.event("agent_start", json!({ "type": "agent_start" }))
        .await;
    let settled = json!({ "type": "agent_settled", "aborted": true });
    host.event("agent_settled", settled).await;
    host.release("llm.complete", verdict("Missing.\n<disapproved/>"))
        .await;
    host.drain(QUIET).await;
    let (summary, _) = host.tool("get_goal", json!({})).await;
    assert!(summary.contains("paused"), "{summary}");
    assert!(
        audit_messages(&host).is_empty(),
        "{:?}",
        audit_messages(&host)
    );
    let continued = host.called("session.sendMessage");
    assert!(continued.is_empty(), "{continued:?}");
}

#[tokio::test]
async fn clear_discards_a_late_rejection() {
    let (mut host, _work, _agent) = start("clear").await;
    submit(&mut host).await;
    command(&mut host, "goal-clear").await;
    host.release("llm.complete", verdict("Missing.\n<disapproved/>"))
        .await;
    host.drain(QUIET).await;
    let (summary, _) = host.tool("get_goal", json!({})).await;
    assert_eq!(summary, "No focused goal.");
    assert!(
        audit_messages(&host).is_empty(),
        "{:?}",
        audit_messages(&host)
    );
}

#[tokio::test]
async fn only_the_latest_audit_applies() {
    let (mut host, _work, _agent) = start("resubmit").await;
    submit(&mut host).await;
    command(&mut host, "goal-pause").await;
    command(&mut host, "goal-resume").await;
    submit(&mut host).await;
    host.release("llm.complete", verdict("<approved/>")).await;
    host.drain(QUIET).await;
    let (summary, _) = host.tool("get_goal", json!({})).await;
    assert!(summary.contains("auditing"), "{summary}");
    assert!(
        audit_messages(&host).is_empty(),
        "{:?}",
        audit_messages(&host)
    );

    host.release("llm.complete", verdict("Missing.\n<disapproved/>"))
        .await;
    host.drain(QUIET).await;
    let messages = audit_messages(&host);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("[GOAL AUDIT REJECTED"), "{messages:?}");
    let (summary, _) = host.tool("get_goal", json!({})).await;
    assert!(summary.contains("running"), "{summary}");
}

#[tokio::test]
async fn pause_during_submit_refresh_cancels_the_audit() {
    let (mut host, _work, _agent) = start("window").await;
    host.hold = vec!["ui.setStatus", "llm.complete"];
    let ctx = host.ctx.clone();
    let call = json!({ "toolCallId": "call-complete", "name": "update_goal",
        "params": complete(), "ctx": ctx });
    let submitted = host.begin("tool.execute", call).await;
    host.wait_for("ui.setStatus").await;
    command(&mut host, "goal-pause").await;
    let audits = host.called("llm.complete").len();
    host.release("ui.setStatus", Value::Null).await;
    host.finish(submitted).await;
    host.drain(QUIET).await;
    assert_eq!(host.called("llm.complete").len(), audits);
    let (summary, _) = host.tool("get_goal", json!({})).await;
    assert!(summary.contains("paused"), "{summary}");
    assert!(
        audit_messages(&host).is_empty(),
        "{:?}",
        audit_messages(&host)
    );
}

#[tokio::test]
async fn audits_of_two_goals_run_independently() {
    let (mut host, _work, _agent) = start("two").await;
    submit(&mut host).await;
    create(&mut host, "Publish the version 2 blog post").await;
    submit(&mut host).await;
    host.release("llm.complete", verdict("Missing.\n<disapproved/>"))
        .await;
    host.drain(QUIET).await;
    let messages = audit_messages(&host);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("[GOAL AUDIT REJECTED"), "{messages:?}");
    let (summary, _) = host.tool("get_goal", json!({})).await;
    assert!(
        summary.contains("blog post") && summary.contains("auditing"),
        "{summary}"
    );

    host.release("llm.complete", verdict("<approved/>")).await;
    host.drain(QUIET).await;
    let messages = audit_messages(&host);
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(messages[1].starts_with("Goal complete"), "{messages:?}");
}
