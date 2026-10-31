//! The compactor's `[Active Work]` block of the focused goal on `compactor:active-work`, from the
//! state the widget shows. Each message names its session, so a late answer cannot outlive a switch.

use std::sync::Arc;

use pi_ext_sdk::prelude::*;
use tokio::sync::Mutex;

use crate::app::{App, Inner};
use crate::format::{objective_title, status_label, truncate_text};
use crate::state::{Goal, GoalStatus};

pub const CHANNEL: &str = "compactor:active-work";
pub const SYNC: &str = "compactor:active-work:sync";
pub const ID: &str = "goal";
const OPEN_TASKS: usize = 8;

/// The session and text of the last message.
type Sent = Option<(Option<String>, String)>;

/// The text last sent, with its session, so an unchanged state sends nothing.
#[derive(Clone, Default)]
pub struct Published(Arc<Mutex<Sent>>);

impl Published {
    /// Sends `text()` unless this session already has it; `force` sends it anyway. The lock spans
    /// reading and sending, so the last message always has the newest state.
    pub async fn publish(&self, host: &Host, text: impl FnOnce() -> String, force: bool) {
        let mut last = self.0.lock().await;
        let session = host.snapshot().and_then(|s| s.session_id.clone());
        let key = (session.clone(), text());
        if !force && last.as_ref() == Some(&key) {
            return;
        }
        let data = json!({ "id": ID, "text": key.1, "sessionId": session });
        *last = Some(key);
        if let Err(err) = host.emit(CHANNEL, data).await {
            pi_ext_sdk::debug!("cannot publish active work: {err}");
        }
    }
}

fn blocked_reason(goal: &Goal) -> Option<&str> {
    let entry = goal
        .history
        .iter()
        .rev()
        .find(|entry| entry.event.starts_with("blocked: "))?;
    Some(entry.event.trim_start_matches("blocked: "))
}

fn goal_text(goal: &Goal) -> String {
    let mut lines = vec![
        format!(
            "Goal: {}",
            truncate_text(&objective_title(&goal.objective), 160)
        ),
        format!("Status: {}", status_label(goal)),
    ];
    let reason = blocked_reason(goal).filter(|_| goal.status == GoalStatus::Blocked);
    if let Some(reason) = reason {
        lines.push(format!("Blocked: {}", truncate_text(reason, 200)));
    }
    if let Some(wait) = &goal.wait {
        lines.push(format!(
            "Waiting until {}: {}",
            wait.deadline,
            truncate_text(&wait.reason, 160)
        ));
    }
    let current = goal.current_task_id.as_deref().and_then(|id| goal.task(id));
    if let Some(task) = current {
        lines.push(format!("Now: {}", task.title));
    }
    let open: Vec<String> = goal
        .ordered_tasks()
        .into_iter()
        .filter(|(_, task)| !task.status.is_resolved())
        .take(OPEN_TASKS)
        .map(|(depth, task)| {
            format!(
                "{}- {} {}",
                "  ".repeat(depth),
                task.status.glyph(),
                task.title
            )
        })
        .collect();
    if !open.is_empty() {
        lines.push("Open tasks:".into());
        lines.extend(open);
    }
    lines.join("\n")
}

/// The block text: the focused open goal, a draft in progress, or empty.
pub fn text(inner: &Inner) -> String {
    if inner.drafting() {
        return "Goal: drafting a new goal with the user".into();
    }
    match inner.focused().filter(|goal| goal.status.is_open()) {
        Some(goal) => goal_text(goal),
        None => String::new(),
    }
}

pub async fn publish(app: &App, host: &Host, force: bool) {
    app.published()
        .publish(host, || text(&app.lock()), force)
        .await;
}

/// Sends the current block once, then again for each sync request of the compactor.
pub async fn serve(app: App, host: Host) -> Result<()> {
    let mut sync = host.subscribe(SYNC).await?;
    publish(&app, &host, true).await;
    while sync.recv().await.is_some() {
        publish(&app, &host, true).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StopReason;

    fn inner_with(goal: Goal) -> Inner {
        Inner {
            focus: Some(goal.id.clone()),
            goals: vec![goal],
            ..Inner::default()
        }
    }

    #[test]
    fn the_focused_goal_shows_status_reason_and_open_tasks() {
        let mut goal = crate::tests::goal_with_tasks();
        goal.current_task_id = Some(goal.tasks[0].id.clone());
        goal.set_status(GoalStatus::Blocked, Some(StopReason::Agent));
        goal.log("blocked: the API key is missing; tried: env");
        let text = text(&inner_with(goal));
        assert!(
            text.starts_with("Goal: Ship it\nStatus: blocked\nBlocked: the API key is missing"),
            "{text}"
        );
        assert!(
            text.contains("Now: ") && text.contains("Open tasks:\n- [ ] "),
            "{text}"
        );
    }

    #[test]
    fn a_complete_or_unfocused_goal_has_no_block() {
        let mut goal = Goal::new("Done already", false, None);
        goal.set_status(GoalStatus::Complete, None);
        assert_eq!(text(&inner_with(goal)), "");
        assert_eq!(text(&Inner::default()), "");
    }
}
