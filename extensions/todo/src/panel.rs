//! `/todos` panel: key handling and the panel loop.

use pi_ext_sdk::{Host, Key, PanelEvent, Result};
use serde_json::{Value, json};

use crate::model::TaskStatus;
use crate::panel_view::{Mode, PanelState, Purpose, Row, render, rows};
use crate::params::Action;
use crate::scopes::Visibility;
use crate::store::{Runtime, Store};

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    None,
    Close,
    Apply(Action, Value),
    Visibility(String, Visibility),
    Collapse,
}

fn input(panel: &mut PanelState, purpose: Purpose, prefill: &str) -> Effect {
    panel.mode = Mode::Input {
        purpose,
        buffer: prefill.to_owned(),
    };
    Effect::None
}

fn phase_of(row: &Row, runtime: &Runtime) -> Option<u64> {
    match row {
        Row::Phase(id) => Some(*id),
        Row::Task(id) => runtime.state.task(*id).and_then(|t| t.phase),
        _ => None,
    }
}

fn activate(panel: &mut PanelState, row: &Row, runtime: &Runtime) -> Effect {
    let state = &runtime.state;
    match row {
        Row::Task(id) => match state.task(*id).map(|t| t.status) {
            Some(TaskStatus::Pending) => {
                Effect::Apply(Action::Update, json!({ "id": id, "status": "in_progress" }))
            }
            Some(TaskStatus::InProgress) => {
                Effect::Apply(Action::Update, json!({ "id": id, "status": "completed" }))
            }
            _ => {
                panel.message = "Completed todos stay completed; d deletes".into();
                Effect::None
            }
        },
        Row::Phase(id) => match state.phase(*id) {
            Some(p) => Effect::Apply(
                Action::UpdatePhase,
                json!({ "phase": id, "status": p.status.next().as_str() }),
            ),
            None => Effect::None,
        },
        Row::Scope(id) => {
            let current = runtime
                .scopes
                .visibility(id, runtime.config.default_visibility);
            Effect::Visibility(id.clone(), current.next())
        }
        Row::NoPhase => Effect::None,
    }
}

/// `up`: false moves down. Tasks move within their phase group, phases among phases.
fn reorder(row: &Row, runtime: &Runtime, up: bool) -> Effect {
    let state = &runtime.state;
    match row {
        Row::Task(id) => {
            let phase = state.task(*id).and_then(|t| t.phase);
            let group: Vec<u64> = state
                .visible_tasks()
                .filter(|t| t.phase == phase)
                .map(|t| t.id)
                .collect();
            let at = group.iter().position(|t| t == id).unwrap_or(0);
            let neighbor = if up { at.checked_sub(1) } else { Some(at + 1) };
            match neighbor.and_then(|n| group.get(n)) {
                Some(other) => {
                    let position = state.tasks.iter().position(|t| t.id == *other).unwrap_or(0) + 1;
                    Effect::Apply(Action::Move, json!({ "id": id, "position": position }))
                }
                None => Effect::None,
            }
        }
        Row::Phase(id) => {
            let at = state.phases.iter().position(|p| p.id == *id).unwrap_or(0);
            let target = if up {
                at.checked_sub(1)
            } else {
                Some(at + 1).filter(|t| *t < state.phases.len())
            };
            match target {
                Some(t) => Effect::Apply(
                    Action::UpdatePhase,
                    json!({ "phase": id, "position": t + 1 }),
                ),
                None => Effect::None,
            }
        }
        _ => Effect::None,
    }
}

fn submit(purpose: &Purpose, text: &str) -> Effect {
    let text = text.trim();
    let apply = |action, args| Effect::Apply(action, args);
    match purpose {
        Purpose::Goal(id) => apply(Action::SetGoal, json!({ "phase": id, "goal": text })),
        _ if text.is_empty() => Effect::None,
        Purpose::AddTask(phase) => {
            apply(Action::Create, json!({ "subject": text, "phase": phase }))
        }
        Purpose::AddPhase => apply(Action::AddPhase, json!({ "title": text })),
        Purpose::EditTask(id) => apply(Action::Update, json!({ "id": id, "subject": text })),
        Purpose::EditPhase(id) => apply(Action::UpdatePhase, json!({ "phase": id, "title": text })),
    }
}

fn input_key(panel: &mut PanelState, key: &Key) -> Effect {
    let Mode::Input { purpose, buffer } = &mut panel.mode else {
        return Effect::None;
    };
    match key {
        Key::Enter => {
            let effect = submit(purpose, buffer);
            panel.mode = Mode::Normal;
            effect
        }
        Key::Escape | Key::Ctrl('c') => {
            panel.mode = Mode::Normal;
            panel.message = "Cancelled".into();
            Effect::None
        }
        Key::Backspace => {
            buffer.pop();
            Effect::None
        }
        Key::Ctrl('u') => {
            buffer.clear();
            Effect::None
        }
        Key::Char(c) => {
            buffer.push(*c);
            Effect::None
        }
        Key::Text(text) => {
            buffer.push_str(&text.replace(['\r', '\n'], " "));
            Effect::None
        }
        _ => Effect::None,
    }
}

pub fn handle_key(panel: &mut PanelState, key: &Key, runtime: &Runtime) -> Effect {
    match panel.mode.clone() {
        Mode::Input { .. } => return input_key(panel, key),
        Mode::ConfirmDelete(row) => {
            panel.mode = Mode::Normal;
            return match (key.is("y"), row) {
                (true, Row::Task(id)) => Effect::Apply(Action::Delete, json!({ "id": id })),
                (true, Row::Phase(id)) => {
                    Effect::Apply(Action::DeletePhase, json!({ "phase": id }))
                }
                _ => {
                    panel.message = "Cancelled".into();
                    Effect::None
                }
            };
        }
        Mode::Normal => {}
    }
    let all = rows(runtime);
    let row = all.get(panel.cursor).cloned();
    let state = &runtime.state;
    match (key, row) {
        (Key::Up | Key::Char('k'), _) => panel.cursor = panel.cursor.saturating_sub(1),
        (Key::Down | Key::Char('j'), _) => {
            panel.cursor = (panel.cursor + 1).min(all.len().saturating_sub(1))
        }
        (Key::Char('K'), Some(row)) => return reorder(&row, runtime, true),
        (Key::Char('J'), Some(row)) => return reorder(&row, runtime, false),
        (Key::Enter | Key::Char(' '), Some(row)) => return activate(panel, &row, runtime),
        (Key::Char('v'), Some(row @ Row::Scope(_))) => return activate(panel, &row, runtime),
        (Key::Char('a'), row) => {
            let phase = row.and_then(|r| phase_of(&r, runtime));
            return input(panel, Purpose::AddTask(phase), "");
        }
        (Key::Char('p'), _) => return input(panel, Purpose::AddPhase, ""),
        (Key::Char('e'), Some(Row::Task(id))) => {
            let subject = state
                .task(id)
                .map(|t| t.subject.clone())
                .unwrap_or_default();
            return input(panel, Purpose::EditTask(id), &subject);
        }
        (Key::Char('e'), Some(Row::Phase(id))) => {
            let title = state.phase(id).map(|p| p.title.clone()).unwrap_or_default();
            return input(panel, Purpose::EditPhase(id), &title);
        }
        (Key::Char('g'), Some(row)) => match phase_of(&row, runtime) {
            Some(id) => {
                let goal = state
                    .phase(id)
                    .and_then(|p| p.goal.clone())
                    .unwrap_or_default();
                return input(panel, Purpose::Goal(id), &goal);
            }
            None => panel.message = "Select a phase to set its goal".into(),
        },
        (Key::Char('d'), Some(row @ (Row::Task(_) | Row::Phase(_)))) => {
            panel.mode = Mode::ConfirmDelete(row)
        }
        (Key::Char('c'), _) => return Effect::Collapse,
        (key, _) if key.is_cancel() => return Effect::Close,
        _ => {}
    }
    Effect::None
}

pub async fn run(store: Store, host: Host) -> Result<()> {
    let mut ui = PanelState::default();
    let lines = render(&ui, &store.lock(), usize::MAX);
    let mut panel = host.panel(lines).open().await?;
    let mut width = usize::MAX;
    loop {
        match panel.next_event().await {
            PanelEvent::Key(key) => {
                let effect = handle_key(&mut ui, &key, &store.lock());
                match effect {
                    Effect::Close => return panel.close(Value::Null).await.map(drop),
                    Effect::Apply(action, args) => {
                        ui.message = store.apply(&host, action, args).await?
                    }
                    Effect::Visibility(scope, visibility) => {
                        store.set_visibility(&host, &scope, visibility).await?;
                        ui.message = format!("Visibility: {}", visibility.as_str());
                    }
                    Effect::Collapse => {
                        store.toggle_collapse(&host).await?;
                        ui.message = "Toggled the widget".into();
                    }
                    Effect::None => {}
                }
            }
            PanelEvent::Resize(w) => width = w,
            PanelEvent::Closed => return Ok(()),
        }
        let lines = {
            let runtime = store.lock();
            ui.cursor = ui.cursor.min(rows(&runtime).len().saturating_sub(1));
            render(&ui, &runtime, width)
        };
        panel.set_lines(lines)?;
    }
}
