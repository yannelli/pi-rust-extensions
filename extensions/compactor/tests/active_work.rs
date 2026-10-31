//! Compactor summaries receive current work from real goal and background-task extensions.

mod common;
#[path = "common/active_work_bus.rs"]
mod relay;

use relay::{COMPACTOR, GOAL, TASKS, bus};
use serde_json::json;
use std::time::Duration;

/// The id of the first running background task in an `[Active Work]` section.
fn task_id(active: &str) -> String {
    let mut lines = active
        .lines()
        .skip_while(|line| !line.starts_with("Background tasks running"));
    let task = lines.nth(1).expect("a running task line");
    task.trim_start_matches("- ")
        .split(' ')
        .next()
        .unwrap()
        .to_owned()
}

fn dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("compactor-active-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test(flavor = "multi_thread")]
async fn producers_fill_active_work_and_completed_or_switched_work_leaves() {
    let root = dir("lifecycle");
    let mut bus = bus(&root).await;
    bus.session_start("s1", "startup").await;
    assert_eq!(
        bus.active_work().await,
        "",
        "a new session has no active work"
    );

    let created = bus
        .tool(
            GOAL,
            "create_goal",
            json!({ "objective": "Ship the parser rewrite" }),
        )
        .await;
    assert!(
        !created.to_string().contains("\"isError\":true"),
        "{created}"
    );
    let started = bus
        .tool(
            TASKS,
            "bg_run",
            json!({ "name": "Watch", "command": "sleep 30", "isAgent": false }),
        )
        .await;
    assert!(
        started
            .to_string()
            .contains("Started background task Watch"),
        "{started}"
    );
    bus.settle(Duration::from_millis(300)).await;
    let active = bus.active_work().await;
    assert!(active.contains("Goal: Ship the parser rewrite"), "{active}");
    assert!(
        active.contains("Background tasks running (1):") && active.contains("Watch"),
        "{active}"
    );

    bus.tool(TASKS, "bg_kill", json!({ "taskId": task_id(&active) }))
        .await;
    bus.settle(Duration::from_millis(1500)).await;
    let active = bus.active_work().await;
    assert!(
        !active.contains("Background tasks"),
        "a killed task stayed: {active}"
    );
    assert!(active.contains("Ship the parser rewrite"), "{active}");

    bus.session_start("s2", "new").await;
    let late = json!({ "id": "goal", "text": "Goal: from s1", "sessionId": "s1" });
    let note = json!({ "jsonrpc": "2.0", "method": "events.message", "params": { "channel": "compactor:active-work", "data": late } });
    bus.send(COMPACTOR, note).await;
    bus.settle(Duration::from_millis(200)).await;
    assert_eq!(
        bus.active_work().await,
        "",
        "work of s1 stayed after the switch"
    );
    bus.stop_tasks().await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn work_sent_before_the_compactor_subscribed_is_still_seen() {
    let root = dir("early");
    let mut bus = bus(&root).await;
    bus.ctx["sessionId"] = json!("s1");
    let start = json!({ "type": "session_start", "reason": "startup" });
    let params = json!({ "name": "session_start", "event": start, "ctx": bus.ctx });
    bus.request(TASKS, "event", params.clone()).await;
    bus.tool(
        TASKS,
        "bg_run",
        json!({ "name": "Build", "command": "sleep 30", "isAgent": false }),
    )
    .await;
    bus.settle(Duration::from_millis(300)).await;
    bus.request(COMPACTOR, "event", params).await;
    bus.settle(Duration::from_millis(300)).await;
    let active = bus.active_work().await;
    assert!(
        active.contains("Background tasks running (1):") && active.contains("Build"),
        "{active}"
    );
    bus.stop_tasks().await;
    let _ = std::fs::remove_dir_all(&root);
}
