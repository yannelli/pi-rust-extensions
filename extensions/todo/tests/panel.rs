//! `/todos` panel key handling.

use pi_ext_sdk::Key;
use pi_ext_todo::model::{Phase, PhaseStatus, Task, TodoState};
use pi_ext_todo::panel::{Effect, handle_key};
use pi_ext_todo::panel_view::{Mode, PanelState, Purpose, Row, rows};
use pi_ext_todo::params::Action;
use pi_ext_todo::scopes::{ScopeEvent, Visibility};
use pi_ext_todo::store::Runtime;
use serde_json::json;

fn runtime() -> Runtime {
    let mut state = TodoState::default();
    state.phases.push(Phase {
        id: 1,
        title: "Design".into(),
        goal: None,
        status: PhaseStatus::Pending,
    });
    for id in 1..=2 {
        let mut task = Task::new(id, format!("Task {id}"));
        task.phase = Some(1);
        state.tasks.push(task);
    }
    state.next_id = 3;
    let mut runtime = Runtime::default();
    runtime.state = state;
    let event: ScopeEvent =
        serde_json::from_value(json!({ "scope": "r1", "label": "build: x", "status": "running" }))
            .unwrap();
    runtime.scopes.apply(event);
    runtime
}

fn press(panel: &mut PanelState, runtime: &Runtime, keys: &[&str]) -> Effect {
    keys.iter().fold(Effect::None, |_, raw| {
        handle_key(panel, &Key::parse(raw), runtime)
    })
}

#[test]
fn rows_follow_phase_order_then_scopes() {
    let rt = runtime();
    assert_eq!(
        rows(&rt),
        [
            Row::Phase(1),
            Row::Task(1),
            Row::Task(2),
            Row::NoPhase,
            Row::Scope("r1".into())
        ]
    );
}

#[test]
fn enter_cycles_task_and_phase_status() {
    let rt = runtime();
    let mut panel = PanelState::default();
    assert_eq!(
        press(&mut panel, &rt, &["\r"]),
        Effect::Apply(
            Action::UpdatePhase,
            json!({ "phase": 1, "status": "in_progress" })
        )
    );
    assert_eq!(
        press(&mut panel, &rt, &["j", " "]),
        Effect::Apply(Action::Update, json!({ "id": 1, "status": "in_progress" }))
    );
}

#[test]
fn reorder_goal_delete_and_visibility() {
    let rt = runtime();
    let mut panel = PanelState {
        cursor: 2,
        ..PanelState::default()
    };
    assert_eq!(
        press(&mut panel, &rt, &["K"]),
        Effect::Apply(Action::Move, json!({ "id": 2, "position": 1 }))
    );
    assert_eq!(press(&mut panel, &rt, &["J"]), Effect::None);
    press(&mut panel, &rt, &["g"]);
    assert_eq!(
        panel.mode,
        Mode::Input {
            purpose: Purpose::Goal(1),
            buffer: String::new()
        }
    );
    assert_eq!(
        press(&mut panel, &rt, &["ship", "\r"]),
        Effect::Apply(Action::SetGoal, json!({ "phase": 1, "goal": "ship" }))
    );
    assert_eq!(
        press(&mut panel, &rt, &["d", "y"]),
        Effect::Apply(Action::Delete, json!({ "id": 2 }))
    );
    assert_eq!(press(&mut panel, &rt, &["d", "n"]), Effect::None);
    panel.cursor = 4;
    assert_eq!(
        press(&mut panel, &rt, &["v"]),
        Effect::Visibility("r1".into(), Visibility::MainOnly)
    );
}

#[test]
fn input_mode_edits_and_cancels() {
    let rt = runtime();
    let mut panel = PanelState {
        cursor: 1,
        ..PanelState::default()
    };
    press(&mut panel, &rt, &["e"]);
    assert_eq!(
        panel.mode,
        Mode::Input {
            purpose: Purpose::EditTask(1),
            buffer: "Task 1".into()
        }
    );
    assert_eq!(
        press(&mut panel, &rt, &["\x7f", "q", "\r"]),
        Effect::Apply(Action::Update, json!({ "id": 1, "subject": "Task q" }))
    );
    press(&mut panel, &rt, &["a", "x", "\x1b"]);
    assert_eq!(panel.mode, Mode::Normal);
    assert_eq!(press(&mut panel, &rt, &["q"]), Effect::Close);
}
