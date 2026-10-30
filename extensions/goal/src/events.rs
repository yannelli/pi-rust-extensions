use std::time::Instant;

use pi_ext_sdk::prelude::*;

use crate::app::App;
use crate::state::{GoalStatus, StopReason, now_ms};
use crate::{draft, prompts, scheduler, widget};

pub const SECTION: &str = "goal";

/// `session_start`, `session_tree`, `session_compact`: replay state and redraw.
pub async fn on_session(app: &App, host: &Host) -> Result<()> {
    app.reload(host).await?;
    if let Err(err) = app.install_profile(host).await {
        pi_ext_sdk::warn!("goal tool profile: {}", err.message());
    }
    let wait = app
        .lock()
        .focused()
        .filter(|g| g.status == GoalStatus::Active)
        .and_then(|g| g.wait.as_ref().map(|w| (g.id.clone(), w.deadline_ms)));
    if let Some((goal_id, deadline_ms)) = wait {
        scheduler::schedule_wait(app, host, goal_id, deadline_ms);
    }
    widget::refresh(app, host).await
}

pub async fn before_agent_start(app: &App, ev: &EventCtx) -> Result<EventReply> {
    if let Err(err) = app.install_profile(ev.host()).await {
        pi_ext_sdk::warn!("goal tool profile: {}", err.message());
    }
    let mut inner = app.lock();
    inner.run.stopped = false;
    inner.run.tool_calls = 0;
    let section = if let Some(draft) = &inner.draft {
        Some(draft::draft_section(draft))
    } else if let Some(goal) = inner.focused().filter(|g| g.status.is_open()) {
        Some(prompts::goal_prompt(goal, &inner.settings))
    } else {
        let open = inner.open_goals().count();
        (open > 0 && !inner.settings.hide_unfocused_prompt).then(|| prompts::unfocused_prompt(open))
    };
    Ok(EventReply::none().with_section(SECTION, section))
}

pub fn tool_call(app: &App, ev: &EventCtx) -> EventReply {
    let name = ev
        .get("toolName")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut inner = app.lock();
    if inner.drafting() && name == "create_goal" {
        return EventReply::block(
            "create_goal is not a shortcut during drafting; call propose_goal_draft.",
        );
    }
    if inner.run.stopped && name != "get_goal" {
        let goal = inner.focused();
        let status = goal.map_or("stopped".into(), crate::format::status_label);
        let why = goal
            .and_then(prompts::stop_note)
            .map(|note| format!(": {note}"))
            .unwrap_or_default();
        return EventReply::block(format!(
            "The goal stopped this turn ({status}{why}). Do not continue work; reply briefly and end the turn."
        ));
    }
    if name != "get_goal" {
        inner.run.tool_calls += 1;
    }
    EventReply::none()
}

pub async fn message_end(app: &App, ev: &EventCtx) -> Result<()> {
    let message = ev.get("message");
    if message.and_then(|m| m.get("role")).and_then(Value::as_str) != Some("assistant") {
        return Ok(());
    }
    let tokens = message
        .and_then(|m| m.pointer("/usage/totalTokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if tokens == 0 {
        return Ok(());
    }
    let limited = {
        let mut inner = app.lock();
        let limited = inner.update_focused(|g| {
            if !matches!(g.status, GoalStatus::Active | GoalStatus::Auditing) {
                return false;
            }
            g.usage.tokens_used += tokens;
            let hit = g.status == GoalStatus::Active && g.budget_reached();
            if hit {
                g.set_status(GoalStatus::BudgetLimited, Some(StopReason::Agent));
                g.log("token budget reached");
            }
            hit
        });
        let limited = limited.unwrap_or(false);
        inner.run.stopped |= limited;
        limited
    };
    if limited {
        let host = ev.host();
        host.notify_warning("goal stopped: token budget reached; /goal-tweak raises it")
            .await?;
    }
    widget::refresh(app, ev.host()).await
}

pub fn agent_start(app: &App) {
    let mut inner = app.lock();
    inner.run.started = Some(Instant::now());
}

pub fn agent_end(app: &App) {
    let mut inner = app.lock();
    let Some(started) = inner.run.started else {
        return;
    };
    let seconds = started.elapsed().as_secs();
    inner.update_focused(|g| {
        if g.status.is_open() {
            g.usage.active_seconds += seconds;
            g.updated_at = now_ms();
        }
    });
}
