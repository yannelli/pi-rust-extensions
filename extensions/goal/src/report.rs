//! `/goal-list`, `/goal-status`, and `/goal-recovery`.

use pi_ext_sdk::prelude::*;

use crate::app::App;
use crate::format::{objective_title, status_label};
use crate::view;

pub fn list(app: &App) -> String {
    let inner = app.lock();
    let lines: Vec<String> = inner
        .open_goals()
        .map(|g| {
            let mark = if inner.focus.as_deref() == Some(g.id.as_str()) {
                "*"
            } else {
                " "
            };
            format!(
                "{mark} {} [{}] {}",
                g.id,
                status_label(g),
                objective_title(&g.objective)
            )
        })
        .collect();
    if lines.is_empty() {
        "No open goals.".into()
    } else {
        format!("Open goals:\n{}", lines.join("\n"))
    }
}

pub fn status(app: &App, args: &str) -> String {
    let inner = app.lock();
    if args == "health" {
        let root = inner
            .store
            .as_ref()
            .map(|s| s.root().display().to_string())
            .unwrap_or_default();
        return format!(
            "Goal files: {root}\nOpen goals: {}\nUnreadable files: {} (see /goal-recovery)\nDraft open: {}",
            inner.open_goals().count(),
            inner.unreadable,
            inner.drafting()
        );
    }
    match inner.focused() {
        Some(goal) if args == "verbose" => view::verbose(goal),
        Some(goal) => view::summary(goal, true),
        None => format!(
            "No focused goal. {} open goal(s).",
            inner.open_goals().count()
        ),
    }
}

pub async fn recovery(app: &App, host: &Host, args: &str) -> Result<()> {
    let Some(store) = app.lock().store.clone() else {
        return Ok(());
    };
    let broken = store.list().unreadable;
    if broken.is_empty() {
        return host.notify_info("goal: all goal files are readable").await;
    }
    let names: Vec<String> = broken.iter().map(|p| p.display().to_string()).collect();
    if args != "repair" {
        return host
            .notify_warning(format!(
                "Unreadable goal files:\n{}\nRun /goal-recovery repair to move them aside.",
                names.join("\n")
            ))
            .await;
    }
    if host.has_ui()
        && !host
            .confirm(
                "Repair goal files?",
                &format!("Move {} unreadable file(s) to archive/?", broken.len()),
            )
            .await?
    {
        return Ok(());
    }
    for path in &broken {
        if let Err(err) = store.quarantine(path) {
            host.notify_error(format!("cannot move {}: {err}", path.display()))
                .await?;
        }
    }
    crate::events::on_session(app, host).await?;
    host.notify_info("goal recovery done").await
}
