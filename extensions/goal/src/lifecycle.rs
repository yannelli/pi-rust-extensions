use pi_ext_sdk::prelude::*;
use tokio::sync::oneshot;

use crate::app::{AUDIT_MESSAGE, App, AuditJob, Inner};
use crate::auditor::{self, Verdict};
use crate::format::truncate_text;
use crate::prompts::pending_tasks;
use crate::scheduler;
use crate::state::{Goal, GoalStatus, GoalTask, StopReason};
use crate::widget;

#[derive(Debug, Clone, Default)]
pub struct NewGoal {
    pub objective: String,
    pub sisyphus: bool,
    pub token_budget: Option<u64>,
    pub tasks: Vec<GoalTask>,
    pub block_completion: bool,
    pub auto_continue: Option<bool>,
}

pub fn validate_objective(app: &App, objective: &str) -> std::result::Result<(), String> {
    if objective.trim().is_empty() {
        return Err("objective must not be empty.".into());
    }
    let max = app.lock().settings.objective_max_chars.filter(|m| *m > 0);
    match max {
        Some(max) if objective.chars().count() > max => {
            Err(format!("objective exceeds objectiveMaxChars ({max})."))
        }
        _ => Ok(()),
    }
}

/// Creates, saves, and focuses a goal; other open goals stay open.
pub async fn create(app: &App, host: &Host, new: NewGoal) -> std::result::Result<Goal, String> {
    validate_objective(app, &new.objective)?;
    if new.token_budget == Some(0) {
        return Err("token_budget must be a positive whole number.".into());
    }
    let mut goal = Goal::new(&new.objective, new.sisyphus, new.token_budget);
    goal.tasks = new.tasks;
    goal.block_completion = new.block_completion;
    goal.auto_continue = new.auto_continue.unwrap_or(true);
    {
        let mut inner = app.lock();
        inner.save(&goal);
        inner.goals.push(goal.clone());
        inner.run.idle_runs = 0;
        inner.run.limit_notified = false;
    }
    app.set_focus(host, Some(goal.id.clone()))
        .await
        .map_err(|e| e.message().to_owned())?;
    let _ = app.install_profile(host).await;
    let _ = widget::refresh(app, host).await;
    Ok(goal)
}

/// `update_goal({status: "complete"})`: gate on tasks, then wait for the auditor and return its
/// verdict as the tool result, as upstream's tool does.
pub async fn submit_completion(app: &App, host: &Host, summary: Option<String>) -> ToolResult {
    let (inline, pending) = {
        let mut inner = app.lock();
        let Some(goal) = inner.focused().cloned() else {
            return ToolResult::error("No focused goal.");
        };
        let gating = goal.gating_pending();
        if !gating.is_empty() {
            return ToolResult::error(format!(
                "TASK GATE: the goal stays open until these tasks are complete or skipped with a reason: {}. Finish them with update_goal_task, then request completion again.",
                pending_tasks(&gating)
            ));
        }
        inner.run.stopped = true;
        inner.update_focused(|g| {
            g.completion_summary = summary.clone();
            g.set_status(GoalStatus::Auditing, None);
            g.log("completion requested");
        });
        let goal = inner.focused().cloned().unwrap_or(goal);
        if inner.settings.disable_auditor {
            (Some(goal.id), None)
        } else {
            (
                None,
                Some(start_audit(app, &mut inner, host, goal, summary)),
            )
        }
    };
    if let Some(goal_id) = inline {
        let verdict = Verdict {
            approved: true,
            report: "Auditor disabled.".into(),
            error: None,
        };
        finish_audit(app, host, &goal_id, None, verdict).await;
        return ToolResult::text("Goal complete and archived (auditor disabled). End this turn.");
    }
    let _ = widget::refresh(app, host).await;
    let Some(verdict) = pending else {
        return ToolResult::error("No audit started.");
    };
    match verdict.await {
        Ok(message) => {
            let mut inner = app.lock();
            let reopened = inner
                .focused()
                .is_some_and(|g| g.status == GoalStatus::Active);
            inner.run.stopped = !reopened;
            ToolResult::text(message.content.as_str().unwrap_or_default())
        }
        Err(_) => ToolResult::text(
            "The audit stopped before a verdict (paused, cleared, aborted, or replaced). The goal stays open.",
        ),
    }
}

/// Spawns the auditor for `goal` and records it while the caller holds the lock. The verdict
/// message goes to the returned receiver, or to the session when nothing waits for it.
fn start_audit(
    app: &App,
    inner: &mut Inner,
    host: &Host,
    goal: Goal,
    summary: Option<String>,
) -> oneshot::Receiver<CustomMessage> {
    inner.cancel_audit(&goal.id);
    inner.audit_generation += 1;
    let generation = inner.audit_generation;
    let goal_id = goal.id.clone();
    let settings = inner.settings.clone();
    let (app, host) = (app.clone(), host.clone());
    let (sender, receiver) = oneshot::channel();
    let task = tokio::spawn(async move {
        let verdict = auditor::audit(&host, &goal, summary.as_deref(), &settings).await;
        let Some(message) = apply_verdict(&app, &host, &goal.id, Some(generation), verdict).await
        else {
            return;
        };
        if let Err(message) = sender.send(message) {
            deliver(&host, message).await;
        }
    });
    let abort = task.abort_handle();
    inner.audits.insert(goal_id, AuditJob { generation, abort });
    receiver
}

/// Applies a verdict and sends its message to the session.
pub async fn finish_audit(
    app: &App,
    host: &Host,
    goal_id: &str,
    generation: Option<u64>,
    verdict: Verdict,
) {
    if let Some(message) = apply_verdict(app, host, goal_id, generation, verdict).await {
        deliver(host, message).await;
    }
}

async fn deliver(host: &Host, message: CustomMessage) {
    if let Err(err) = host.send_message(message).await {
        pi_ext_sdk::warn!("goal audit message failed: {}", err.message());
    }
}

/// Applies a current verdict; a goal change or newer audit makes an older verdict stale.
/// A `generation` of `None` applies an inline verdict.
async fn apply_verdict(
    app: &App,
    host: &Host,
    goal_id: &str,
    generation: Option<u64>,
    verdict: Verdict,
) -> Option<CustomMessage> {
    let message = {
        let mut inner = app.lock();
        if let Some(generation) = generation {
            if inner.audits.get(goal_id).map(|job| job.generation) != Some(generation) {
                return None;
            }
            inner.audits.remove(goal_id);
        }
        let goal = inner
            .goals
            .iter_mut()
            .find(|g| g.id == goal_id && g.status == GoalStatus::Auditing)?;
        goal.audits += 1;
        goal.last_audit = Some(verdict.report.clone());
        if verdict.approved {
            goal.set_status(GoalStatus::Complete, None);
            goal.log("auditor approved");
            inner.archive(goal_id);
            CustomMessage::new(
                AUDIT_MESSAGE,
                "Goal complete: the auditor approved it and the goal is archived.",
            )
            .display(true)
        } else {
            let (status, text) = match &verdict.error {
                Some(error) => (
                    GoalStatus::Paused,
                    format!(
                        "Goal audit failed: {error}. The goal is paused; /goal-resume retries."
                    ),
                ),
                None => (
                    GoalStatus::Active,
                    format!(
                        "[GOAL AUDIT REJECTED goalId={goal_id}]\nThe completion auditor disapproved. Address the findings, then request completion again.\n\n{}",
                        truncate_text_block(&verdict.report)
                    ),
                ),
            };
            goal.set_status(status, verdict.error.as_ref().map(|_| StopReason::Agent));
            goal.log(match &verdict.error {
                Some(error) => format!("auditor failed: {}", truncate_text(error, 200)),
                None => "auditor disapproved".into(),
            });
            let goal = goal.clone();
            let auto = goal.auto_continue && status == GoalStatus::Active;
            inner.save(&goal);
            let message = CustomMessage::new(AUDIT_MESSAGE, text).display(true);
            if auto {
                message.trigger_turn(true).deliver_as(DeliverAs::FollowUp)
            } else {
                message
            }
        }
    };
    let focus_cleared = app.lock().focus.is_none();
    if verdict.approved && focus_cleared {
        let _ = app.set_focus(host, None).await;
    }
    let _ = widget::refresh(app, host).await;
    Some(message)
}

fn truncate_text_block(report: &str) -> String {
    let cut: String = report.chars().take(4_000).collect();
    cut.trim().to_owned()
}

pub fn pause_focused(app: &App, reason: StopReason, why: &str) -> Option<Goal> {
    let mut inner = app.lock();
    let paused = inner.update_focused(|g| {
        if matches!(g.status, GoalStatus::Active | GoalStatus::Auditing) {
            g.set_status(GoalStatus::Paused, Some(reason));
            g.log(format!("paused: {}", truncate_text(why, 200)));
        }
        g.clone()
    });
    inner.cancel_focused_audit();
    inner.run.stopped = true;
    paused
}

/// Reactivates the focused goal, renews its run allowance, and sends a continuation.
pub async fn resume(app: &App, host: &Host) -> std::result::Result<Goal, String> {
    let goal = {
        let mut inner = app.lock();
        inner.run.idle_runs = 0;
        inner.run.limit_notified = false;
        inner.cancel_focused_audit();
        inner
            .update_focused(|g| {
                g.set_status(GoalStatus::Active, None);
                g.runs = 0;
                g.wait = None;
                g.log("resumed");
                g.clone()
            })
            .ok_or("No focused goal; /goal-focus picks one.")?
    };
    widget::refresh(app, host)
        .await
        .map_err(|e| e.message().to_owned())?;
    if goal.budget_reached() {
        return Err("The token budget is used up; /goal-tweak raises it.".into());
    }
    scheduler::send_continuation(app, host, &goal.id)
        .await
        .map_err(|e| e.message().to_owned())?;
    Ok(goal)
}

pub async fn clear(app: &App, host: &Host) -> Option<Goal> {
    let goal = {
        let mut inner = app.lock();
        let id = inner.focus.clone()?;
        inner.cancel_focused_audit();
        inner.update_focused(|g| g.log("cleared by user"));
        inner.archive(&id)
    };
    let _ = app.set_focus(host, None).await;
    let _ = app.install_profile(host).await;
    let _ = widget::refresh(app, host).await;
    goal
}
