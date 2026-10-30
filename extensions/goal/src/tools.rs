use pi_ext_sdk::prelude::*;
use serde::Deserialize;

use crate::app::App;
use crate::format::parse_iso_ms;
use crate::lifecycle::{self, NewGoal};
use crate::state::{GoalStatus, StopReason, WaitState, now_ms};
use crate::tasks::{self, FlatTask};
use crate::update::{UpdatePlan, plan_update_goal};
use crate::{prompts, scheduler, view, widget};

#[derive(Deserialize)]
pub struct CreateArgs {
    objective: String,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    token_budget: Option<u64>,
}

pub async fn create_goal(app: App, args: CreateArgs, ctx: ToolCtx) -> Result<ToolResult> {
    if app.lock().drafting() {
        return Ok(ToolResult::error(
            "create_goal is not a shortcut during drafting; call propose_goal_draft so the user confirms the goal.",
        ));
    }
    let new = NewGoal {
        objective: args.objective,
        sisyphus: args.mode.as_deref() == Some("sisyphus"),
        token_budget: args.token_budget,
        ..NewGoal::default()
    };
    Ok(match lifecycle::create(&app, ctx.host(), new).await {
        Ok(goal) => ToolResult::text(format!(
            "Goal created and focused (goalId={}).\n\n{}",
            goal.id,
            view::summary(&goal, false)
        ))
        .details(json!({ "goalId": goal.id })),
        Err(err) => ToolResult::error(err),
    })
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct GetArgs {
    section: Option<String>,
    task_id: Option<String>,
    cursor: Option<String>,
    verbose: bool,
    include_history: bool,
}

pub fn get_goal(app: &App, args: GetArgs) -> ToolResult {
    let inner = app.lock();
    let Some(goal) = inner.focused() else {
        let open = inner.open_goals().count();
        return ToolResult::text(if open > 0 {
            format!("No focused goal. {open} open goal(s); ask the user to run /goal-focus.")
        } else {
            "No focused goal.".into()
        });
    };
    let text = match (args.section.as_deref(), args.verbose) {
        (_, true) => view::verbose(goal),
        (Some("objective"), _) => view::objective(goal),
        (Some("tasks"), _) => match view::tasks(goal, args.task_id.as_deref()) {
            Ok(text) => text,
            Err(err) => return ToolResult::error(err),
        },
        (Some("history"), _) => view::history(goal),
        _ if args.include_history => {
            format!("{}\n\n{}", view::summary(goal, true), view::history(goal))
        }
        _ => view::summary(goal, true),
    };
    match view::page(&text, args.cursor.as_deref()) {
        Ok(page) => ToolResult::text(page),
        Err(err) => ToolResult::error(err),
    }
}

pub async fn update_goal(app: App, args: Value, ctx: ToolCtx) -> Result<ToolResult> {
    let host = ctx.host();
    let plan = match plan_update_goal(&args) {
        Ok(plan) => plan,
        Err(err) => return Ok(ToolResult::error(err)),
    };
    let focused = app.lock().focused().cloned();
    match focused {
        None => return Ok(ToolResult::error("No focused goal.")),
        Some(goal)
            if !matches!(
                goal.status,
                GoalStatus::Active | GoalStatus::Paused | GoalStatus::BudgetLimited
            ) =>
        {
            let why = prompts::stop_note(&goal)
                .map(|note| format!(": {note}"))
                .unwrap_or_default();
            return Ok(ToolResult::error(format!(
                "The goal is {}{why}. update_goal does not apply.",
                crate::format::status_label(&goal)
            )));
        }
        Some(_) => {}
    }
    let text = match plan {
        UpdatePlan::Complete { summary } => {
            return Ok(lifecycle::submit_completion(&app, host, summary).await);
        }
        UpdatePlan::Paused {
            reason,
            suggested_action,
        } => {
            lifecycle::pause_focused(&app, StopReason::Agent, &reason);
            let hint = suggested_action
                .map(|s| format!(" Suggested: {s}"))
                .unwrap_or_default();
            let _ = host
                .notify_warning(format!("goal paused by the agent: {reason}{hint}"))
                .await;
            "Goal paused. End this turn; the user resumes with /goal-resume.".to_owned()
        }
        UpdatePlan::Blocked {
            reason,
            attempted_actions,
        } => {
            {
                let mut inner = app.lock();
                inner.run.stopped = true;
                inner.update_focused(|g| {
                    g.set_status(GoalStatus::Blocked, Some(StopReason::Agent));
                    g.log(format!(
                        "blocked: {reason}; tried: {}",
                        attempted_actions.join("; ")
                    ));
                });
            }
            let _ = host.notify_warning(format!("goal blocked: {reason}")).await;
            "Goal marked blocked. End this turn and report the blocker to the user.".to_owned()
        }
        UpdatePlan::Ready => {
            app.lock().update_focused(|g| g.wait = None);
            "Continuation ready; the next autonomous run continues the goal.".to_owned()
        }
        UpdatePlan::Wait(wait) => {
            if !app.lock().settings.strict_execution_contract {
                return Ok(ToolResult::error(
                    "New waits require strictExecutionContract opt-in; continue working instead.",
                ));
            }
            let Some(deadline_ms) = parse_iso_ms(&wait.deadline).filter(|ms| *ms > now_ms()) else {
                return Ok(ToolResult::error(
                    "continuation.wait.deadline must be a future ISO 8601 time.",
                ));
            };
            let goal_id = {
                let mut inner = app.lock();
                inner.run.stopped = true;
                inner.update_focused(|g| {
                    g.wait = Some(WaitState {
                        reason: wait.reason.clone(),
                        deadline: wait.deadline.clone(),
                        deadline_ms,
                        wait_id: wait.wait_id.clone(),
                    });
                    g.log(format!("wait until {}: {}", wait.deadline, wait.reason));
                    g.id.clone()
                })
            };
            if let Some(goal_id) = goal_id {
                scheduler::schedule_wait(&app, host, goal_id, deadline_ms);
            }
            format!(
                "Wait saved until {}. End this turn; the goal resumes at the deadline or on pi-goal:wake.",
                wait.deadline
            )
        }
    };
    let _ = widget::refresh(&app, host).await;
    Ok(ToolResult::text(text))
}

#[derive(Deserialize)]
pub struct SetTasksArgs {
    tasks: Vec<FlatTask>,
    #[serde(default)]
    block_completion: Option<bool>,
    #[serde(default)]
    change_summary: Option<String>,
}

pub async fn set_goal_tasks(app: App, args: SetTasksArgs, ctx: ToolCtx) -> Result<ToolResult> {
    let host = ctx.host();
    let merged = {
        let inner = app.lock();
        if !inner.tasks_enabled() {
            return Ok(ToolResult::error("Tasks are disabled (disableTasks)."));
        }
        let Some(goal) = inner.focused() else {
            return Ok(ToolResult::error("No focused goal."));
        };
        if !matches!(goal.status, GoalStatus::Active | GoalStatus::Paused) {
            return Ok(ToolResult::error(
                "Tasks change only on an active or paused goal.",
            ));
        }
        tasks::merge_tasks(&args.tasks, &goal.tasks)
    };
    let merged = match merged {
        Ok(merged) => merged,
        Err(err) => return Ok(ToolResult::error(err)),
    };
    if ctx.snapshot().has_ui && ctx.snapshot().mode.as_deref() != Some("rpc") {
        let mut preview = crate::draft::render_flat(&merged);
        if let Some(summary) = &args.change_summary {
            preview = format!("{summary}\n\n{preview}");
        }
        if !host
            .confirm("Apply goal task list?", &preview)
            .await
            .unwrap_or(false)
        {
            return Ok(ToolResult::text(
                "The user declined the task list. Ask what to change.",
            ));
        }
    }
    let count = merged.len();
    app.lock().update_focused(|g| {
        g.tasks = merged;
        if let Some(block) = args.block_completion {
            g.block_completion = block;
        }
        if g.current_task_id
            .as_deref()
            .is_some_and(|id| g.task(id).is_none())
        {
            g.current_task_id = None;
        }
        g.log(format!("task list set ({count} tasks)"));
    });
    let _ = widget::refresh(&app, host).await;
    Ok(ToolResult::text(format!(
        "Task list saved ({count} tasks). End this turn; continuation follows."
    )))
}

pub async fn update_goal_task(app: App, args: Value, ctx: ToolCtx) -> Result<ToolResult> {
    let updates = match tasks::normalize_task_progress(&args) {
        Ok(updates) => updates,
        Err(err) => return Ok(ToolResult::error(err)),
    };
    let result = {
        let mut inner = app.lock();
        match inner.focused().map(|g| g.status) {
            None => Err("No focused goal.".to_owned()),
            Some(GoalStatus::Active) => inner
                .update_focused(|g| {
                    tasks::apply_progress(g, &updates).map(|notes| (notes, g.counts()))
                })
                .unwrap_or_else(|| Err("No focused goal.".into())),
            Some(_) => Err("Task progress changes only while the goal is active.".into()),
        }
    };
    let _ = widget::refresh(&app, ctx.host()).await;
    Ok(match result {
        Ok((notes, counts)) => ToolResult::text(format!(
            "Updated {}. Tasks: {}/{} complete.",
            notes.join(", "),
            counts.complete,
            counts.total
        )),
        Err(err) => ToolResult::error(err),
    })
}
