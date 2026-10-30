//! Replay from branch entries and widget line rendering.

use std::collections::HashSet;

use pi_ext_sdk::style::strip;
use pi_ext_todo::config::Config;
use pi_ext_todo::model::{Task, TaskStatus, TodoState};
use pi_ext_todo::replay::replay;
use pi_ext_todo::scopes::{ScopeEvent, Scopes, Visibility};
use pi_ext_todo::widget::{View, render};
use serde_json::{Value, json};

fn state(tasks: &[(&str, TaskStatus)]) -> TodoState {
    let mut state = TodoState::default();
    for (i, (subject, status)) in tasks.iter().enumerate() {
        let mut task = Task::new(i as u64 + 1, *subject);
        task.status = *status;
        state.tasks.push(task);
    }
    state.next_id = tasks.len() as u64 + 1;
    state
}

fn lines(
    state: &TodoState,
    scopes: &Scopes,
    config: &Config,
    collapsed: bool,
    width: usize,
) -> Vec<String> {
    let empty = HashSet::new();
    let hidden_scopes = HashSet::new();
    let view = View {
        state,
        hidden: &empty,
        collapsed,
        scopes,
        hidden_scopes: &hidden_scopes,
        config,
    };
    render(&view, width).iter().map(|l| strip(l)).collect()
}

fn details(subject: &str) -> Value {
    json!({ "action": "create", "params": {}, "nextId": 2, "tasks": [{ "id": 1, "subject": subject, "status": "pending" }] })
}

#[test]
fn replay_prefers_state_entries_then_tool_results() {
    let tool_result = json!({ "type": "message", "message": { "role": "toolResult", "toolName": "todo", "details": details("from result") } });
    let (state, _) = replay(std::slice::from_ref(&tool_result));
    assert_eq!(state.tasks[0].subject, "from result");
    let entry =
        json!({ "type": "custom", "customType": "rpiv-todo-state", "data": details("from entry") });
    let bad = json!({ "type": "custom", "customType": "rpiv-todo-state", "data": { "tasks": ["x"], "nextId": 1 } });
    let (state, _) = replay(&[entry, tool_result, bad]);
    assert_eq!(state.tasks[0].subject, "from entry");
    assert_eq!(replay(&[]).0, TodoState::default());
}

#[test]
fn replay_restores_scopes() {
    let mut scopes = Scopes::default();
    let event: ScopeEvent = serde_json::from_value(json!({ "scope": "r1", "label": "explore: scan", "status": "running", "state": details("child") })).unwrap();
    scopes.apply(event);
    scopes.set_visibility("r1", Visibility::All);
    let entry = json!({ "type": "custom", "customType": "pi-todo-scopes", "data": scopes });
    let (_, replayed) = replay(&[entry]);
    assert_eq!(replayed, scopes);
}

#[test]
fn expanded_widget_lists_tasks() {
    let state = state(&[
        ("Write parser", TaskStatus::InProgress),
        ("Test", TaskStatus::Pending),
        ("Plan", TaskStatus::Completed),
    ]);
    let got = lines(&state, &Scopes::default(), &Config::default(), false, 80);
    assert_eq!(
        got,
        [
            "● Todos (1/3)",
            "├─ ◐ Write parser",
            "├─ ○ Test",
            "└─ ✓ Plan",
            ""
        ]
    );
}

#[test]
fn collapsed_widget_shows_the_expand_hint() {
    let state = state(&[("A", TaskStatus::Pending)]);
    let got = lines(&state, &Scopes::default(), &Config::default(), true, 80);
    assert_eq!(got, ["● Todos (0/1)", "└─ ctrl+shift+t to expand", ""]);
}

#[test]
fn overflow_drops_completed_first_and_truncates() {
    let tasks: Vec<(&str, TaskStatus)> = vec![("done", TaskStatus::Completed); 3]
        .into_iter()
        .chain(vec![("open", TaskStatus::Pending); 4])
        .collect();
    let state = state(&tasks);
    let config = Config {
        max_widget_lines: 4,
        ..Config::default()
    };
    let got = lines(&state, &Scopes::default(), &config, false, 80);
    assert_eq!(
        got,
        [
            "● Todos (3/7)",
            "├─ ○ open",
            "├─ ○ open",
            "└─ +5 more (3 completed, 2 pending)",
            ""
        ]
    );
    let narrow = lines(&state, &Scopes::default(), &config, false, 8);
    assert_eq!(narrow[1], "├─ ○ op…");
}

#[test]
fn phase_heading_goal_and_current_phase_tasks() {
    let mut state = state(&[
        ("Sketch", TaskStatus::Completed),
        ("Code", TaskStatus::Pending),
    ]);
    state.phases = serde_json::from_value(json!([
        { "id": 1, "title": "Design", "status": "completed" },
        { "id": 2, "title": "Build", "goal": "ship v1", "status": "in_progress" }
    ]))
    .unwrap();
    state.tasks[0].phase = Some(1);
    state.tasks[1].phase = Some(2);
    let got = lines(&state, &Scopes::default(), &Config::default(), false, 80);
    assert_eq!(
        got,
        [
            "● Todos (0/1) · Phase 2/2: Build",
            "├─ Goal: ship v1",
            "└─ ○ Code",
            ""
        ]
    );
}

#[test]
fn scope_visibility_modes() {
    let main = state(&[("Main task", TaskStatus::Pending)]);
    let mut scopes = Scopes::default();
    let event: ScopeEvent = serde_json::from_value(json!({ "scope": "r1", "label": "explore: scan", "status": "running", "state": details("Child task") })).unwrap();
    scopes.apply(event);
    let all = Config {
        default_visibility: Visibility::All,
        ..Config::default()
    };
    let got = lines(&main, &scopes, &all, false, 80);
    assert_eq!(
        got[2..],
        ["◆ explore: scan · 0/1 · running", "  └─ ○ Child task", ""]
    );
    let got = lines(&main, &scopes, &Config::default(), false, 80);
    assert_eq!(got[2..], ["◆ explore: scan · 0/1 · running", ""]);
    scopes.set_visibility("r1", Visibility::MainOnly);
    let got = lines(&main, &scopes, &all, false, 80);
    assert_eq!(got, ["● Todos (0/1)", "└─ ○ Main task", ""]);
    let empty = lines(&TodoState::default(), &Scopes::default(), &all, false, 80);
    assert!(empty.is_empty());
}
