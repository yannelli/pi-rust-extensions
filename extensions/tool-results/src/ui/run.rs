//! Opens the `/tool-results` panel and applies its edits to the global file.

use pi_ext_sdk::InputOptions;
use pi_ext_sdk::prelude::*;

use super::render::Inputs;
use super::{Action, Screen, View, rule_overrides};
use crate::config::field_def;
use crate::{Shared, files, lock, reload};

/// Rows pi keeps outside a panel.
const HOST_ROWS: usize = 3;

enum Next {
    Close,
    /// Close the panel for a dialog, then open it again.
    Dialog(Action),
}

fn draw(state: &Shared, view: &View, width: usize, height: Option<usize>) -> Vec<String> {
    let session = lock(state);
    let note = session.project.as_ref().map(|p| {
        format!(
            "{} overrides the global file; the panel edits the global file.",
            p.display()
        )
    });
    let inputs = Inputs {
        config: &session.config,
        stats: &session.stats,
        note: note.as_deref(),
    };
    view.render(&inputs, width, height)
}

pub(crate) async fn open(state: Shared, ctx: &CommandCtx) -> Result<()> {
    let (host, snap) = (ctx.host(), ctx.snapshot());
    let mut view = View::new();
    let mut width = snap.terminal_width.map_or(100, |w| w as usize);
    let mut height = snap
        .terminal_height
        .map(|h| (h as usize).saturating_sub(HOST_ROWS));
    loop {
        let mut panel = host
            .panel(draw(&state, &view, width, height))
            .open()
            .await?;
        width = panel.width().unwrap_or(width);
        let next = loop {
            match panel.next_event().await {
                PanelEvent::Closed => return Ok(()),
                PanelEvent::Resize(w) => {
                    width = w;
                    height = panel
                        .height()
                        .map(|h| h.saturating_sub(HOST_ROWS))
                        .or(height);
                }
                PanelEvent::Key(key) => {
                    let (config, tools) = {
                        let session = lock(&state);
                        (session.config.clone(), session.stats.total.len())
                    };
                    match view.key(&key, &config, tools) {
                        Action::Close => break Next::Close,
                        Action::Redraw => {}
                        action @ (Action::EditNumber { .. } | Action::AddRule) => {
                            break Next::Dialog(action);
                        }
                        action => view.status = Some(apply(&state, ctx, action, None).await),
                    }
                }
            }
            clamp(&state, &mut view);
            panel.set_lines(draw(&state, &view, width, height))?;
        };
        let _ = panel.close(Value::Null).await;
        let Next::Dialog(action) = next else {
            return Ok(());
        };
        let answer = ask(&state, host, &action).await?;
        if let Some(answer) = answer {
            if matches!(action, Action::AddRule) && !answer.trim().is_empty() {
                view.screen = Screen::Rule(answer.trim().to_owned());
                view.selected = 0;
            }
            view.status = Some(apply(&state, ctx, action, Some(answer)).await);
        }
        clamp(&state, &mut view);
    }
}

fn clamp(state: &Shared, view: &mut View) {
    let session = lock(state);
    view.clamp(&session.config, session.stats.total.len());
}

async fn ask(state: &Shared, host: &Host, action: &Action) -> Result<Option<String>> {
    match action {
        Action::EditNumber { rule, key } => {
            let label = field_def(key).map_or(*key, |f| f.label.trim());
            let current = {
                let config = &lock(state).config;
                match rule {
                    Some(pattern) => {
                        let (user, builtin) = rule_overrides(config, pattern);
                        user.get(*key).or(builtin.get(*key)).map(Value::to_string)
                    }
                    None => Some(config.get(key).to_string()),
                }
            };
            let title = match rule {
                Some(pattern) => format!("{label} for {pattern} (empty inherits)"),
                None => label.to_owned(),
            };
            let options = InputOptions {
                prefill: current,
                ..InputOptions::default()
            };
            host.input_with(&title, options).await
        }
        Action::AddRule => {
            host.input(
                "Tool name or glob, for example mcp__* or bash",
                Some("bash"),
            )
            .await
        }
        _ => Ok(None),
    }
}

/// Writes the change, reloads the config, and returns the status line.
async fn apply(state: &Shared, ctx: &CommandCtx, action: Action, answer: Option<String>) -> String {
    let host = ctx.host();
    let path = files::global_path(&host.agent_dir());
    let result = match action {
        Action::Set { rule, key, value } => {
            let shown = value
                .as_ref()
                .map_or("inherit".to_owned(), Value::to_string);
            files::edit(&path, rule.as_deref(), key, value).map(|()| format!("{key}: {shown}"))
        }
        Action::EditNumber { rule, key } => {
            let text = answer.unwrap_or_default();
            let text = text.trim();
            let value = match text {
                "" if rule.is_some() => None,
                _ => match text.replace([',', '_'], "").parse::<u64>() {
                    Ok(n) => Some(json!(n)),
                    Err(_) => return format!("{text:?} is not a number."),
                },
            };
            let shown = value
                .as_ref()
                .map_or("inherit".to_owned(), Value::to_string);
            files::edit(&path, rule.as_deref(), key, value).map(|()| format!("{key}: {shown}"))
        }
        Action::AddRule => {
            let pattern = answer.unwrap_or_default();
            let pattern = pattern.trim();
            if pattern.is_empty() {
                return "No rule added.".into();
            }
            files::edit_rule(&path, pattern, false).map(|()| format!("Rule {pattern} added."))
        }
        Action::RemoveRule(pattern) => {
            files::edit_rule(&path, &pattern, true).map(|()| format!("Rule {pattern} removed."))
        }
        Action::ResetStats => {
            let mut session = lock(state);
            session.stats.reset();
            return match session.stats.save(&crate::stats::path(&host.agent_dir())) {
                Ok(()) => "Stats cleared.".into(),
                Err(err) => format!("Stats cleared; cannot write the stats file: {err}"),
            };
        }
        Action::Redraw | Action::Close => return String::new(),
    };
    reload(state, host, ctx.snapshot()).await;
    match result {
        Ok(text) => format!("{text} (saved to {})", path.display()),
        Err(err) => format!("Cannot write {}: {err}", path.display()),
    }
}
