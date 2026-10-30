//! Task state machine, dependency checks, batch create, phases, and result text.

use pi_ext_todo::format::content;
use pi_ext_todo::model::TodoState;
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
fn create_update_delete_texts() {
    let mut state = TodoState::default();
    assert_eq!(
        step(
            &mut state,
            json!({ "action": "create", "subject": "Write the parser" })
        ),
        "Created #1: Write the parser (pending)"
    );
    assert_eq!(
        step(
            &mut state,
            json!({ "action": "update", "id": 1, "status": "in_progress", "activeForm": "writing" })
        ),
        "Updated #1 (pending → in_progress)"
    );
    assert_eq!(
        step(
            &mut state,
            json!({ "action": "update", "id": 1, "status": "in_progress" })
        ),
        "No change: #1 already matches the requested values (status: in_progress)"
    );
    assert_eq!(
        step(
            &mut state,
            json!({ "action": "update", "id": 1, "owner": "me" })
        ),
        "Updated #1"
    );
    assert_eq!(
        step(&mut state, json!({ "action": "list" })),
        "[in_progress] #1 Write the parser (writing)"
    );
    assert_eq!(
        step(&mut state, json!({ "action": "delete", "id": 1 })),
        "Deleted #1: Write the parser"
    );
    assert_eq!(
        step(&mut state, json!({ "action": "delete", "id": 1 })),
        "Error: #1 is already deleted"
    );
    assert_eq!(step(&mut state, json!({ "action": "list" })), "No tasks");
    assert!(
        step(
            &mut state,
            json!({ "action": "list", "includeDeleted": true })
        )
        .starts_with("[deleted] #1")
    );
    assert_eq!(
        step(&mut state, json!({ "action": "clear" })),
        "Cleared 1 tasks"
    );
    assert_eq!(state.next_id, 1);
}

#[test]
fn illegal_transitions_and_missing_fields() {
    let mut state = seeded(1);
    step(
        &mut state,
        json!({ "action": "update", "id": 1, "status": "completed" }),
    );
    assert_eq!(
        step(
            &mut state,
            json!({ "action": "update", "id": 1, "status": "in_progress" })
        ),
        "Error: illegal transition completed → in_progress"
    );
    let (_, text, op) = run(&state, json!({ "action": "update", "id": 1 }));
    assert!(text.starts_with("Error: update requires at least one mutable field"));
    assert!(!op.changes_state());
    assert_eq!(
        run(&state, json!({ "action": "update" })).1,
        "Error: id required for update"
    );
    assert_eq!(
        run(&state, json!({ "action": "get", "id": 9 })).1,
        "Error: #9 not found"
    );
    assert_eq!(
        run(&state, json!({ "action": "create" })).1,
        "Error: subject required for create"
    );
}

#[test]
fn dependency_checks_reject_before_mutation() {
    let mut state = seeded(3);
    assert_eq!(
        run(
            &state,
            json!({ "action": "create", "subject": "X", "blockedBy": [9] })
        )
        .1,
        "Error: blockedBy: #9 not found"
    );
    step(
        &mut state,
        json!({ "action": "update", "id": 2, "addBlockedBy": [1] }),
    );
    step(
        &mut state,
        json!({ "action": "update", "id": 3, "addBlockedBy": [2] }),
    );
    assert_eq!(
        run(
            &state,
            json!({ "action": "update", "id": 1, "addBlockedBy": [3] })
        )
        .1,
        "Error: addBlockedBy would create a cycle in the blockedBy graph"
    );
    assert_eq!(
        run(
            &state,
            json!({ "action": "update", "id": 1, "addBlockedBy": [1] })
        )
        .1,
        "Error: cannot block #1 on itself"
    );
    step(&mut state, json!({ "action": "delete", "id": 3 }));
    assert_eq!(
        run(
            &state,
            json!({ "action": "update", "id": 1, "addBlockedBy": [3] })
        )
        .1,
        "Error: addBlockedBy: #3 is deleted"
    );
    let get = run(&state, json!({ "action": "get", "id": 1 })).1;
    assert_eq!(get, "#1 [pending] Task 1\n  blocks: #2");
    step(
        &mut state,
        json!({ "action": "update", "id": 2, "removeBlockedBy": [1] }),
    );
    assert!(state.task(2).unwrap().blocked_by.is_empty());
}

#[test]
fn batch_create_is_all_or_nothing() {
    let state = seeded(1);
    let (next, text, _) = run(
        &state,
        json!({ "action": "create", "tasks": [{ "subject": "A" }, { "subject": "B", "blockedBy": [1] }] }),
    );
    assert_eq!(text, "Created #2: A (pending)\nCreated #3: B (pending)");
    assert_eq!(next.next_id, 4);
    let sibling = run(
        &state,
        json!({ "action": "create", "tasks": [{ "subject": "A" }, { "subject": "B", "blockedBy": [2] }] }),
    );
    assert!(
        sibling
            .1
            .starts_with("Error: blockedBy: #2 is another item in this batch")
    );
    assert_eq!(sibling.0, state);
    let top = run(
        &state,
        json!({ "action": "create", "owner": "x", "tasks": [{ "subject": "A" }] }),
    );
    assert_eq!(
        top.1,
        "Error: create with tasks[] takes owner on each item, not at the top level"
    );
    let both = run(
        &state,
        json!({ "action": "create", "subject": "", "tasks": [{ "subject": "A" }] }),
    );
    assert_eq!(
        both.1,
        "Error: create requires subject or tasks[], not both"
    );
    assert_eq!(
        run(&state, json!({ "action": "create", "tasks": [] })).1,
        "Error: tasks[] must be non-empty"
    );
    assert_eq!(
        run(
            &state,
            json!({ "action": "create", "tasks": [{ "subject": " " }] })
        )
        .1,
        "Error: tasks[0]: subject required for create"
    );
}

#[test]
fn metadata_merges_and_null_deletes() {
    let mut state = seeded(1);
    step(
        &mut state,
        json!({ "action": "update", "id": 1, "metadata": { "a": 1, "b": 2 } }),
    );
    step(
        &mut state,
        json!({ "action": "update", "id": 1, "metadata": { "a": null } }),
    );
    assert_eq!(
        state.task(1).unwrap().metadata.as_ref().unwrap().get("b"),
        Some(&json!(2))
    );
    step(
        &mut state,
        json!({ "action": "update", "id": 1, "metadata": { "b": null } }),
    );
    assert!(state.task(1).unwrap().metadata.is_none());
}
