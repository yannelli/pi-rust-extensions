//! goal under the pi binary: create a goal, settle, then the autonomous continuation sets and
//! updates tasks, reads the goal, and pauses it.

use pi_ext_e2e::{Run, Turn, cargo_bin, json, pi_bin_or_skip};

const OBJECTIVE: &str = "Write the release notes for version 2";
const TASK: &str = "Draft the release notes";

#[test]
fn goal_continues_with_tasks_and_pauses() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["goal"])
        .env("PI_EXT_LOG", "debug")
        .turns([
            Turn::tool_call("create_goal", json!({"objective": OBJECTIVE})),
            Turn::text("Goal created."),
            Turn::tool_call(
                "set_goal_tasks",
                json!({"tasks": [{"id": "t1", "title": TASK}]}),
            ),
            Turn::tool_call(
                "update_goal_task",
                json!({"task_id": "t1", "status": "start"}),
            ),
            Turn::tool_call("get_goal", json!({})),
            Turn::tool_call(
                "update_goal",
                json!({"status": "paused", "reason": "The user asked to pause after one step."}),
            ),
            Turn::text("Paused the goal."),
        ])
        .prompt("Create a goal to write the release notes for version 2. You may pause it after one step.")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);

    let tools = [
        "create_goal",
        "set_goal_tasks",
        "update_goal_task",
        "get_goal",
        "update_goal",
    ];
    let ran: Vec<&str> = output
        .events_of("tool_execution_end")
        .iter()
        .filter_map(|event| event["toolName"].as_str())
        .collect();
    let evidence = format!(
        "requests={} tools ran={ran:?} offered[last]={:?}\nstderr:\n{}",
        output.requests.len(),
        output.tool_names(output.requests.len().saturating_sub(1)),
        output.stderr
    );
    for tool in tools {
        let results = output.tool_results(tool);
        assert_eq!(
            results.len(),
            1,
            "{tool} ran {} times\n{evidence}",
            results.len()
        );
        assert_ne!(results[0]["isError"], true, "{tool} failed: {}", results[0]);
    }
    let created = output.tool_result_text("create_goal").unwrap_or_default();
    assert!(created.contains(OBJECTIVE), "{created}");
    let tasks = output
        .tool_result_text("set_goal_tasks")
        .unwrap_or_default();
    assert!(tasks.contains("Task list saved (1 tasks)"), "{tasks}");
    let goal = output.tool_result_text("get_goal").unwrap_or_default();
    assert!(goal.contains(OBJECTIVE) && goal.contains(TASK), "{goal}");
    assert_eq!(output.final_text(), "Paused the goal.");
}
