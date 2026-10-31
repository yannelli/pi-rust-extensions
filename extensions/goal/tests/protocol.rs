mod common;

use common::{Host, TestDir};
use serde_json::{Value, json};

const OBJECTIVE: &str = "Write the release notes for version 2";
const TASK: &str = "Draft the release notes";

fn tool_names(init: &Value) -> Vec<String> {
    let tools = init["tools"].as_array().cloned().unwrap_or_default();
    let names = tools.iter().filter_map(|t| t["name"].as_str());
    names.map(String::from).collect()
}

async fn create_with_task(host: &mut Host) {
    let (created, failed) = host
        .tool("create_goal", json!({ "objective": OBJECTIVE }))
        .await;
    assert!(!failed, "{created}");
    assert!(created.contains(OBJECTIVE), "{created}");
    let tasks = json!({ "tasks": [{ "id": "t1", "title": TASK }] });
    let (saved, failed) = host.tool("set_goal_tasks", tasks).await;
    assert!(!failed, "{saved}");
    assert!(saved.contains("Task list saved (1 tasks)"), "{saved}");
}

#[tokio::test]
async fn registers_core_task_and_draft_tools() {
    let work = TestDir::new("reg-work");
    let agent = TestDir::new("reg-agent");
    let host = Host::start(work.path(), agent.path()).await;
    let init = host.init_result();
    let names = tool_names(init);
    for name in [
        "create_goal",
        "get_goal",
        "update_goal",
        "set_goal_tasks",
        "update_goal_task",
        "propose_goal_draft",
    ] {
        assert!(
            names.contains(&name.to_string()),
            "missing {name}: {names:?}"
        );
    }
    let tools = init["tools"].as_array().unwrap();
    let create = tools.iter().find(|t| t["name"] == "create_goal").unwrap();
    assert_eq!(create["parameters"]["required"], json!(["objective"]));
    let draft = tools
        .iter()
        .find(|t| t["name"] == "propose_goal_draft")
        .unwrap();
    assert!(draft["parameters"]["properties"]["objective"].is_object());
}

#[tokio::test]
async fn tracks_tasks_and_pauses() {
    let work = TestDir::new("flow-work");
    let agent = TestDir::new("flow-agent");
    let mut host = Host::start(work.path(), agent.path()).await;
    create_with_task(&mut host).await;
    assert!(!host.called("ui.confirm").is_empty());
    assert!(!host.called("session.appendEntry").is_empty());

    let start = json!({ "task_id": "t1", "status": "start" });
    let (updated, failed) = host.tool("update_goal_task", start).await;
    assert!(!failed, "{updated}");
    assert!(updated.contains("0/1 complete"), "{updated}");
    let (tasks, failed) = host.tool("get_goal", json!({ "section": "tasks" })).await;
    assert!(!failed && tasks.contains(TASK), "{tasks}");
    let (summary, _) = host.tool("get_goal", json!({})).await;
    assert!(summary.contains(OBJECTIVE), "{summary}");
    assert!(summary.contains("Current task: t1"), "{summary}");

    let pause = json!({ "status": "paused", "reason": "The user asked to pause." });
    let (paused, failed) = host.tool("update_goal", pause).await;
    assert!(!failed && paused.starts_with("Goal paused"), "{paused}");
    let (summary, _) = host.tool("get_goal", json!({})).await;
    assert!(summary.contains("paused (agent)"), "{summary}");
    let done = json!({ "task_id": "t1", "status": "complete", "evidence": "notes.md" });
    let (rejected, failed) = host.tool("update_goal_task", done).await;
    assert!(failed, "task progress on a paused goal: {rejected}");
}

async fn submit(host: &mut Host) -> String {
    let done = json!({ "task_id": "t1", "status": "complete", "evidence": "notes.md written" });
    let (updated, failed) = host.tool("update_goal_task", done).await;
    assert!(!failed && updated.contains("1/1 complete"), "{updated}");
    let complete = json!({ "status": "complete", "completion_summary": "Notes written." });
    let (verdict, failed) = host.tool("update_goal", complete).await;
    assert!(!failed, "{verdict}");
    let request = host.called("llm.complete");
    assert_eq!(request.len(), 1);
    let prompt = request[0]["prompt"].as_str().unwrap_or_default();
    assert!(
        prompt.contains(OBJECTIVE) && prompt.contains("Notes written."),
        "{prompt}"
    );
    verdict
}

#[tokio::test]
async fn approved_audit_completes_and_archives() {
    let work = TestDir::new("ok-work");
    let agent = TestDir::new("ok-agent");
    let mut host = Host::start(work.path(), agent.path()).await;
    host.verdict = "Every item is satisfied.\n<approved/>".into();
    create_with_task(&mut host).await;
    let message = submit(&mut host).await;
    assert!(message.starts_with("Goal complete"), "{message}");
    let (summary, _) = host.tool("get_goal", json!({})).await;
    assert_eq!(summary, "No focused goal.");
}

#[tokio::test]
async fn rejected_audit_keeps_goal_open() {
    let work = TestDir::new("no-work");
    let agent = TestDir::new("no-agent");
    let mut host = Host::start(work.path(), agent.path()).await;
    host.verdict = "The notes are missing the upgrade section.\n<disapproved/>".into();
    create_with_task(&mut host).await;
    let message = submit(&mut host).await;
    assert!(message.contains("[GOAL AUDIT REJECTED"), "{message}");
    assert!(message.contains("upgrade section"), "{message}");
    let (summary, _) = host.tool("get_goal", json!({})).await;
    assert!(summary.contains(OBJECTIVE), "{summary}");
    assert!(summary.contains("running"), "{summary}");
}
