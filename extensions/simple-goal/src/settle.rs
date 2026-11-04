//! `agent_settled`: evaluate the active goal, then finish, continue, or pause it.

use pi_ext_sdk::prelude::*;

use crate::evaluator::{self, Verdict};
use crate::state::{App, ENTRY, Goal, Status};

/// Continue verdicts in a row after runs without a tool call before the goal pauses.
pub const MAX_IDLE_STREAK: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settle {
    Nothing,
    PauseAborted,
    Evaluate,
}

/// What `agent_settled` does before any evaluation.
pub fn on_settle(goal: Option<&Goal>, aborted: bool, evaluating: bool) -> Settle {
    match goal {
        Some(goal) if goal.status == Status::Active && aborted => Settle::PauseAborted,
        Some(goal) if goal.status == Status::Active && !evaluating => Settle::Evaluate,
        _ => Settle::Nothing,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Done,
    Continue,
    PauseIdle,
    PauseMaxRounds,
    PauseError(String),
}

/// `tool_calls` counts the calls of the run that was evaluated.
pub fn decide(
    goal: &Goal,
    verdict: &Result<Verdict, String>,
    tool_calls: u32,
    max_rounds: u32,
) -> Decision {
    match verdict {
        Err(err) => Decision::PauseError(err.clone()),
        Ok(verdict) if verdict.done => Decision::Done,
        Ok(_) if next_idle_streak(goal, tool_calls) >= MAX_IDLE_STREAK => Decision::PauseIdle,
        Ok(_) if goal.rounds >= max_rounds => Decision::PauseMaxRounds,
        Ok(_) => Decision::Continue,
    }
}

fn next_idle_streak(goal: &Goal, tool_calls: u32) -> u32 {
    if tool_calls == 0 {
        goal.idle_streak + 1
    } else {
        0
    }
}

/// Applies `decision` to `goal`; `None` when the goal is finished.
pub fn apply(
    mut goal: Goal,
    decision: &Decision,
    verdict: &Result<Verdict, String>,
    tool_calls: u32,
) -> Option<Goal> {
    if let Ok(verdict) = verdict {
        goal.last_reason = Some(verdict.reason.clone());
    }
    match decision {
        Decision::Done => return None,
        Decision::Continue => {
            goal.idle_streak = next_idle_streak(&goal, tool_calls);
            goal.rounds += 1;
        }
        Decision::PauseIdle => goal.pause("no tool calls in recent runs"),
        Decision::PauseMaxRounds => goal.pause("round limit reached"),
        Decision::PauseError(_) => goal.pause("the evaluator failed"),
    }
    Some(goal)
}

pub fn continuation_text(goal: &Goal, reason: &str, max_rounds: u32) -> String {
    let mut lines = vec![
        format!(
            "Simple goal, round {}/{max_rounds}: the evaluator says the objective is not done yet.",
            goal.rounds
        ),
        format!("Objective: {}", goal.objective),
    ];
    if let Some(done_when) = &goal.done_when {
        lines.push(format!("Done when: {done_when}"));
    }
    lines.push(format!("Evaluator: {reason}"));
    lines.push("Keep working toward the objective. If it is impossible or the user should decide, call `simple_goal` with action `clear` and explain why.".into());
    lines.join("\n")
}

pub fn done_text(goal: &Goal, reason: &str) -> String {
    format!("Simple goal done: {}\nEvaluator: {reason}", goal.objective)
}

/// Handles `agent_settled`. The evaluation runs in its own task so the settle event returns at once.
pub async fn on_settled(app: &App, host: &Host, aborted: bool) -> Result<()> {
    let (settle, goal, tool_calls) = {
        let mut inner = app.lock();
        let tool_calls = std::mem::take(&mut inner.tool_calls);
        let settle = on_settle(inner.goal.as_ref(), aborted, inner.evaluating);
        if settle == Settle::Evaluate {
            inner.evaluating = true;
        }
        (settle, inner.goal.clone(), tool_calls)
    };
    let Some(mut goal) = goal else {
        return Ok(());
    };
    match settle {
        Settle::Nothing => Ok(()),
        Settle::PauseAborted => {
            goal.pause("the run was aborted");
            app.save(host, Some(goal)).await?;
            host.notify_warning(
                "simple goal paused after the aborted run; /simple-goal resume continues",
            )
            .await
        }
        Settle::Evaluate => {
            let (app, host) = (app.clone(), host.clone());
            tokio::spawn(async move {
                if let Err(err) = evaluate_and_apply(&app, &host, goal, tool_calls).await {
                    pi_ext_sdk::warn!("simple-goal: {}", err.message());
                }
            });
            Ok(())
        }
    }
}

async fn evaluate_and_apply(app: &App, host: &Host, goal: Goal, tool_calls: u32) -> Result<()> {
    let config = app.config(host).await;
    let verdict = evaluator::evaluate(host, &goal, &config).await;
    let current = {
        let mut inner = app.lock();
        inner.evaluating = false;
        inner.goal.clone()
    };
    if current
        .as_ref()
        .is_none_or(|g| g.id != goal.id || g.status != Status::Active)
    {
        return Ok(());
    }
    let decision = decide(&goal, &verdict, tool_calls, config.max_rounds);
    let next = apply(goal.clone(), &decision, &verdict, tool_calls);
    app.save(host, next.clone()).await?;
    let reason = verdict
        .as_ref()
        .map(|v| v.reason.clone())
        .unwrap_or_default();
    match decision {
        Decision::Done => {
            let text = done_text(&goal, &reason);
            host.notify_info("simple goal done").await?;
            host.send_message(CustomMessage::new(ENTRY, text).display(true))
                .await
        }
        Decision::Continue => {
            let next = next.unwrap_or(goal);
            let text = continuation_text(&next, &reason, config.max_rounds);
            let message = CustomMessage::new(ENTRY, text)
                .display(true)
                .details(json!({ "round": next.rounds, "maxRounds": config.max_rounds }))
                .deliver_as(DeliverAs::FollowUp)
                .trigger_turn(true);
            host.send_message(message).await
        }
        Decision::PauseIdle => {
            host.notify_warning(
                "simple goal paused: no tool calls in recent runs; /simple-goal resume continues",
            )
            .await
        }
        Decision::PauseMaxRounds => {
            let text = format!(
                "simple goal paused after {} rounds; /simple-goal resume continues",
                config.max_rounds
            );
            host.notify_warning(text).await
        }
        Decision::PauseError(err) => {
            let text = format!(
                "simple goal paused: the evaluator failed ({err}); /simple-goal resume continues"
            );
            host.notify_warning(text).await
        }
    }
}
