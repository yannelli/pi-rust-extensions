//! Phase actions and task reordering.

use pi_ext_todo::format::content;
use pi_ext_todo::model::{PhaseStatus, TaskStatus, TodoState};
use pi_ext_todo::params::{parse, parse_action};
use pi_ext_todo::reducer::{Op, apply};
use serde_json::{Value, json};

fn run(state: &TodoState, args: Value) -> (TodoState, String, Op) {
    let action = parse_action(&args).unwrap();
    let (next, op) = apply(state, action, &parse(&args).unwrap());
    let text = content(&op, &next);
    (next, text, op)
}

fn step(state: &mut TodoState, args: Value) -> String {
    let (next, text, _) = run(state, args);
    *state = next;
    text
}

fn seeded(n: usize) -> TodoState {
    let mut state = TodoState::default();
    for i in 1..=n {
        step(
            &mut state,
            json!({ "action": "create", "subject": format!("Task {i}") }),
        );
    }
    state
}

#[test]
fn phases_group_tasks_and_start_with_their_first_task() {
    let mut state = TodoState::default();
    assert_eq!(
        step(
            &mut state,
            json!({ "action": "add_phase", "title": "Design", "goal": "agree on the API" })
        ),
        "Added phase P1: Design (pending)"
    );
    step(
        &mut state,
        json!({ "action": "add_phase", "title": "Build" }),
    );
    step(
        &mut state,
        json!({ "action": "create", "tasks": [{ "subject": "Sketch" }, { "subject": "Code", "phase": 2 }], "phase": 1 }),
    );
    assert_eq!(state.task(1).unwrap().phase, Some(1));
    assert_eq!(state.task(2).unwrap().phase, Some(2));
    assert_eq!(
        run(
            &state,
            json!({ "action": "create", "subject": "X", "phase": 7 })
        )
        .1,
        "Error: phase: P7 not found"
    );
    step(
        &mut state,
        json!({ "action": "update", "id": 1, "status": "in_progress" }),
    );
    assert_eq!(state.phase(1).unwrap().status, PhaseStatus::InProgress);
    assert_eq!(state.current_phase().unwrap().id, 1);
    assert_eq!(
        step(
            &mut state,
            json!({ "action": "update_phase", "phase": 1, "status": "completed" })
        ),
        "Updated phase P1 (in_progress → completed)"
    );
    assert_eq!(state.current_phase().unwrap().id, 2);
    assert_eq!(
        step(
            &mut state,
            json!({ "action": "set_goal", "phase": 2, "goal": "ship" })
        ),
        "Set goal of phase P2: ship"
    );
    assert_eq!(
        step(
            &mut state,
            json!({ "action": "set_goal", "phase": 2, "goal": "" })
        ),
        "Cleared goal of phase P2"
    );
    step(
        &mut state,
        json!({ "action": "update_phase", "phase": 2, "position": 1 }),
    );
    assert_eq!(state.phases[0].id, 2);
    let list = run(&state, json!({ "action": "list" })).1;
    assert_eq!(
        list,
        "── P2 Build [pending] ──\n[pending] #2 Code\n── P1 Design [completed] · goal: agree on the API ──\n[in_progress] #1 Sketch"
    );
    assert_eq!(
        step(&mut state, json!({ "action": "delete_phase", "phase": 2 })),
        "Deleted phase P2: Build (1 tasks moved to no phase)"
    );
    assert_eq!(state.task(2).unwrap().phase, None);
    assert_eq!(state.task(1).unwrap().status, TaskStatus::InProgress);
}

#[test]
fn move_reorders_tasks() {
    let mut state = seeded(3);
    assert_eq!(
        step(
            &mut state,
            json!({ "action": "move", "id": 3, "position": 1 })
        ),
        "Moved #3 to position 1"
    );
    let order: Vec<u64> = state.tasks.iter().map(|t| t.id).collect();
    assert_eq!(order, [3, 1, 2]);
    assert_eq!(
        step(
            &mut state,
            json!({ "action": "move", "id": 3, "position": 1 })
        ),
        "No change: #3 is already at that position"
    );
}
