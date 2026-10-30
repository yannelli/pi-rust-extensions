use std::time::Duration;

use pi_ext_sdk::prelude::*;

use crate::app::{App, EVENT_MESSAGE};
use crate::config::Settings;
use crate::prompts::{continuation_message, continuation_trigger};
use crate::state::{Goal, GoalStatus, StopReason, now_ms};
use crate::widget;

/// Continuation runs in a row without a single tool call before the goal pauses.
pub const MAX_IDLE_RUNS: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Nothing,
    Continue,
    PauseUser,
    PauseIdle,
    BudgetLimited,
    RunLimit,
}

/// What `agent_settled` does for the focused goal.
pub fn decide(
    goal: Option<&Goal>,
    settings: &Settings,
    drafting: bool,
    aborted: bool,
    idle_runs: u32,
) -> Decision {
    let Some(goal) = goal.filter(|_| !drafting) else {
        return Decision::Nothing;
    };
    if aborted && matches!(goal.status, GoalStatus::Active | GoalStatus::Auditing) {
        return Decision::PauseUser;
    }
    if goal.status != GoalStatus::Active {
        return Decision::Nothing;
    }
    if goal.budget_reached() {
        return Decision::BudgetLimited;
    }
    if !goal.auto_continue || goal.wait.as_ref().is_some_and(|w| w.deadline_ms > now_ms()) {
        return Decision::Nothing;
    }
    if settings
        .max_autonomous_runs
        .is_some_and(|limit| goal.runs >= limit)
    {
        return Decision::RunLimit;
    }
    if idle_runs >= MAX_IDLE_RUNS {
        return Decision::PauseIdle;
    }
    Decision::Continue
}

/// Handles `agent_settled`: continue, pause, or stop the focused goal.
pub async fn on_settled(app: &App, host: &Host, aborted: bool) -> Result<()> {
    let (decision, goal_id) = {
        let mut inner = app.lock();
        let idle = if inner.run.tool_calls == 0 && inner.run.started.is_some() {
            inner.run.idle_runs + 1
        } else {
            0
        };
        inner.run.idle_runs = idle;
        let decision = decide(
            inner.focused(),
            &inner.settings,
            inner.drafting(),
            aborted,
            idle,
        );
        let goal_id = inner.focus.clone().unwrap_or_default();
        match decision {
            Decision::PauseUser => pause(&mut inner, StopReason::User, "turn interrupted"),
            Decision::PauseIdle => pause(
                &mut inner,
                StopReason::Agent,
                "no tool calls in recent runs",
            ),
            Decision::BudgetLimited => {
                inner.update_focused(|g| {
                    g.set_status(GoalStatus::BudgetLimited, Some(StopReason::Agent));
                    g.log("token budget reached");
                });
            }
            Decision::RunLimit if inner.run.limit_notified => return Ok(()),
            Decision::RunLimit => inner.run.limit_notified = true,
            Decision::Continue => {
                inner.update_focused(|g| g.runs += 1);
            }
            Decision::Nothing => {}
        }
        (decision, goal_id)
    };
    let notice = match decision {
        Decision::Nothing => return Ok(()),
        Decision::Continue => return send_continuation(app, host, &goal_id).await,
        Decision::PauseUser => "goal paused after the interrupted turn; /goal-resume continues",
        Decision::PauseIdle => "goal paused: no progress in recent runs; /goal-resume continues",
        Decision::BudgetLimited => "goal stopped: token budget reached; /goal-tweak raises it",
        Decision::RunLimit => "goal: autonomous run limit reached; /goal-resume renews it",
    };
    widget::refresh(app, host).await?;
    host.notify_warning(notice).await
}

fn pause(inner: &mut crate::app::Inner, reason: StopReason, why: &str) {
    inner.cancel_focused_audit();
    inner.update_focused(|g| {
        g.set_status(GoalStatus::Paused, Some(reason));
        g.log(format!("paused: {why}"));
    });
}

/// Sends the continuation of `goal_id`, with why the goal is open and what remains.
pub async fn send_continuation(app: &App, host: &Host, goal_id: &str) -> Result<()> {
    let text = {
        let inner = app.lock();
        let goal = inner.goals.iter().find(|goal| goal.id == goal_id);
        goal.map_or_else(
            || continuation_trigger(goal_id),
            |goal| continuation_message(goal, &inner.settings),
        )
    };
    let message = CustomMessage::new(EVENT_MESSAGE, text)
        .display(false)
        .details(json!({ "goalId": goal_id, "kind": "continuation" }))
        .deliver_as(DeliverAs::FollowUp)
        .trigger_turn(true);
    host.send_message(message).await
}

/// Spawns a timer that resumes the goal at its wait deadline unless the wait changed.
pub fn schedule_wait(app: &App, host: &Host, goal_id: String, deadline_ms: u64) {
    let (app, host) = (app.clone(), host.clone());
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(deadline_ms.saturating_sub(now_ms()))).await;
        wake(&app, &host, &goal_id, Some(deadline_ms)).await;
    });
}

/// Ends the wait of `goal_id` and sends a continuation; `deadline` must match when given.
pub async fn wake(app: &App, host: &Host, goal_id: &str, deadline: Option<u64>) {
    let fire = {
        let mut inner = app.lock();
        let matches = inner.focused().is_some_and(|g| {
            g.id == goal_id
                && g.status == GoalStatus::Active
                && g.wait
                    .as_ref()
                    .is_some_and(|w| deadline.is_none_or(|d| w.deadline_ms == d))
        });
        if matches {
            inner.update_focused(|g| {
                g.wait = None;
                g.log("wait ended");
            });
        }
        matches
    };
    if fire {
        if let Err(err) = send_continuation(app, host, goal_id).await {
            pi_ext_sdk::warn!("goal wake failed: {}", err.message());
        }
    }
}
