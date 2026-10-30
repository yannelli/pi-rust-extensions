use pi_ext_sdk::CompletionItem;
use pi_ext_sdk::prelude::*;

use crate::app::{App, DraftMode};
use crate::format::{objective_title, status_label};
use crate::lifecycle::{self, NewGoal};
use crate::state::StopReason;
use crate::{draft, report, scheduler, widget};

pub const COMMANDS: [(&str, &str); 16] = [
    (
        "goal",
        "Draft a goal: start a guided goal with confirmation.",
    ),
    (
        "sisyphus",
        "Draft a Sisyphus goal: an ordered, persistent goal.",
    ),
    (
        "goal-direct",
        "Start a goal directly from the supplied objective.",
    ),
    (
        "sisyphus-direct",
        "Start a Sisyphus goal directly from the supplied objective and ordered steps.",
    ),
    (
        "goal-tweak",
        "Revise the objective or budget, with confirmation.",
    ),
    ("goal-pause", "Pause the goal and stop the current turn."),
    (
        "goal-resume",
        "Continue and renew the autonomous run allowance.",
    ),
    ("goal-clear", "Archive the goal after confirmation."),
    ("goal-list", "Show the open goals in this project."),
    ("goal-status", "Show progress; accepts verbose and health."),
    ("goal-refresh", "Read goal files and settings again."),
    (
        "goal-recovery",
        "Inspect unreadable goal files; repair moves them aside after confirmation.",
    ),
    ("goal-focus", "Choose the goal this session works on."),
    (
        "goal-unfocus",
        "Detach this session, leaving the shared goal open.",
    ),
    ("goal-settings", "Open goal settings."),
    (
        "goal-cancel",
        "Cancel the guided draft without creating a goal.",
    ),
];

pub async fn run(app: App, name: &str, args: String, ctx: CommandCtx) -> Result<()> {
    let host = ctx.host();
    let args = args.trim().to_owned();
    match name {
        "goal" => draft::start(&app, host, DraftMode::Goal, &args).await,
        "sisyphus" => draft::start(&app, host, DraftMode::Sisyphus, &args).await,
        "goal-tweak" => draft::start(&app, host, DraftMode::Tweak, &args).await,
        "goal-direct" => direct(&app, host, args, false).await,
        "sisyphus-direct" => direct(&app, host, args, true).await,
        "goal-cancel" => match draft::cancel(&app, host).await? {
            true => host.notify_info("goal draft cancelled").await,
            false => host.notify_info("goal: no draft is open").await,
        },
        "goal-pause" => pause(&app, host).await,
        "goal-resume" => resume(&app, host, &args).await,
        "goal-clear" => clear(&app, host).await,
        "goal-list" => host.notify_info(report::list(&app)).await,
        "goal-status" => host.notify_info(report::status(&app, &args)).await,
        "goal-refresh" => {
            crate::events::on_session(&app, host).await?;
            host.notify_info("goal state reloaded").await
        }
        "goal-recovery" => report::recovery(&app, host, &args).await,
        "goal-focus" => focus(&app, host, &args).await.map(drop),
        "goal-unfocus" => {
            app.set_focus(host, None).await?;
            app.install_profile(host).await?;
            widget::refresh(&app, host).await?;
            host.notify_info("goal unfocused; it stays open").await
        }
        "goal-settings" => crate::settings_ui::run(&app, host).await,
        _ => Ok(()),
    }
}

async fn direct(app: &App, host: &Host, mut objective: String, sisyphus: bool) -> Result<()> {
    if objective.is_empty() && host.has_ui() {
        let title = if sisyphus {
            "Sisyphus objective and ordered steps"
        } else {
            "Goal objective"
        };
        objective = host.editor(title, None).await?.unwrap_or_default();
    }
    if objective.trim().is_empty() {
        return host.notify_warning("goal: an objective is required").await;
    }
    let new = NewGoal {
        objective,
        sisyphus,
        ..NewGoal::default()
    };
    match lifecycle::create(app, host, new).await {
        Ok(goal) => {
            host.notify_info(format!(
                "goal started: {}",
                objective_title(&goal.objective)
            ))
            .await?;
            if host.is_idle().await.unwrap_or(true) {
                scheduler::send_continuation(host, &goal.id).await?;
            }
            Ok(())
        }
        Err(err) => host.notify_error(format!("goal: {err}")).await,
    }
}

async fn pause(app: &App, host: &Host) -> Result<()> {
    let Some(goal) = lifecycle::pause_focused(app, StopReason::User, "paused by user") else {
        return host.notify_info("goal: no focused goal").await;
    };
    if !host.is_idle().await.unwrap_or(true) {
        host.abort().await?;
    }
    widget::refresh(app, host).await?;
    host.notify_info(format!("goal {}", status_label(&goal)))
        .await
}

async fn resume(app: &App, host: &Host, args: &str) -> Result<()> {
    if app.lock().focus.is_none() && !focus(app, host, args).await? {
        return Ok(());
    }
    match lifecycle::resume(app, host).await {
        Ok(goal) => {
            host.notify_info(format!(
                "goal resumed: {}",
                objective_title(&goal.objective)
            ))
            .await
        }
        Err(err) => host.notify_warning(format!("goal: {err}")).await,
    }
}

async fn clear(app: &App, host: &Host) -> Result<()> {
    let cancelled = draft::cancel(app, host).await?;
    let title = app.lock().focused().map(|g| objective_title(&g.objective));
    let Some(title) = title else {
        let text = if cancelled {
            "goal draft cancelled"
        } else {
            "goal: no focused goal"
        };
        return host.notify_info(text).await;
    };
    if host.has_ui()
        && !host
            .confirm("Clear goal?", &format!("Archive \"{title}\"?"))
            .await?
    {
        return Ok(());
    }
    if !host.is_idle().await.unwrap_or(true) {
        app.lock().run.stopped = true;
    }
    lifecycle::clear(app, host).await;
    host.notify_info(format!("goal archived: {title}")).await
}

/// Focuses the goal named in `args`, or one the user picks; `false` when none was chosen.
async fn focus(app: &App, host: &Host, args: &str) -> Result<bool> {
    let goals: Vec<(String, String)> = app
        .lock()
        .open_goals()
        .map(|g| {
            (
                g.id.clone(),
                format!(
                    "{} [{}] {}",
                    g.id,
                    status_label(g),
                    objective_title(&g.objective)
                ),
            )
        })
        .collect();
    let chosen = match goals.iter().find(|(id, _)| id == args) {
        Some((id, _)) => Some(id.clone()),
        None if goals.is_empty() => None,
        None if goals.len() == 1 && !host.has_ui() => Some(goals[0].0.clone()),
        None if host.has_ui() => {
            let labels: Vec<&str> = goals.iter().map(|(_, l)| l.as_str()).collect();
            let pick = host.select("Focus goal", &labels).await?;
            pick.and_then(|p| {
                goals
                    .iter()
                    .find(|(_, l)| *l == p)
                    .map(|(id, _)| id.clone())
            })
        }
        None => None,
    };
    let Some(id) = chosen else {
        host.notify_info("goal: no open goal chosen").await?;
        return Ok(false);
    };
    app.set_focus(host, Some(id.clone())).await?;
    app.install_profile(host).await?;
    widget::refresh(app, host).await?;
    host.notify_info(format!("goal focused: {id}")).await?;
    Ok(true)
}

pub fn completions(app: &App, name: &str, prefix: &str) -> Option<Vec<CompletionItem>> {
    let values: Vec<String> = match name {
        "goal-status" => vec!["verbose".into(), "health".into()],
        "goal-recovery" => vec!["repair".into()],
        "goal-focus" | "goal-resume" => app.lock().open_goals().map(|g| g.id.clone()).collect(),
        _ => return None,
    };
    Some(
        values
            .into_iter()
            .filter(|v| v.starts_with(prefix))
            .map(CompletionItem::new)
            .collect(),
    )
}
