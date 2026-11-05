//! Opens the `/claude-plugins` panel and runs its operations on a blocking thread, with progress
//! in the status line while git clones or pulls.

use std::time::{Duration, Instant};

use pi_ext_sdk::prelude::*;

use super::{Action, Inventory, Op, Tone, View};
use crate::env::Env;
use crate::{blocking, import, install, market, reconcile};

/// Rows pi keeps outside a panel: the cwd and usage footer lines, and the top row of fullscreen.
const HOST_ROWS: usize = 3;
const TICK: Duration = Duration::from_millis(120);

fn panel_height(rows: Option<usize>) -> Option<usize> {
    rows.map(|rows| rows.saturating_sub(HOST_ROWS).max(1))
}

/// Runs `op` and returns what to show.
pub fn execute(env: &Env, op: &Op) -> anyhow::Result<String> {
    match op.clone() {
        Op::Install { id, scope } => install::install(env, &id, scope),
        Op::Reinstall { id, scope } => install::install(env, &id, scope),
        Op::Update { id, scope } => install::update_one(env, &id, Some(scope)),
        Op::Uninstall { id, scope } => install::uninstall(env, &id, Some(scope)),
        Op::SetEnabled { id, scope, enabled } => {
            install::set_enabled(env, &id, enabled, Some(scope))
        }
        Op::Configure { .. } => Ok(String::new()),
        Op::AddMarket { source, scope } => market::add(env, &source, scope),
        Op::UpdateMarket { name, scope } => {
            let located = market::find(env, &name, Some(scope))?;
            let mut lines = market::update(env, &located)?;
            lines.push(crate::RELOAD.into());
            Ok(lines.join("\n"))
        }
        Op::RemoveMarket { name, scope } => market::remove(env, &name, Some(scope)),
        Op::AutoUpdate { name, on } => market::set_auto_update(env, &name, on),
        Op::UpdateAll => reconcile::update_all(env),
        Op::Import => import::import(env, None),
    }
}

/// Text that reports a failure even when the operation returned Ok (bulk operations list failed
/// items as "Cannot …" lines).
fn tone_of(text: &str) -> Tone {
    if text.lines().any(|line| line.starts_with("Cannot ")) {
        Tone::Warn
    } else {
        Tone::Info
    }
}

pub async fn browse(host: Host, env: Env, trusted: bool) -> Result<()> {
    let mut view = View::new(trusted);
    let snapshot = host.snapshot();
    let mut width = snapshot
        .as_ref()
        .and_then(|s| s.terminal_width)
        .map_or(100, |w| w as usize);
    let mut height = panel_height(
        snapshot
            .as_ref()
            .and_then(|s| s.terminal_height)
            .map(|h| h as usize),
    );
    let mut inv = blocking(env.clone(), |env| Ok(Inventory::read(&env))).await?;
    loop {
        let mut panel = host.panel(view.render(&inv, width, height)).open().await?;
        width = panel.width().unwrap_or(width);
        // `Some(id)`: the panel closes to ask for the plugin's options, then opens again.
        let configure: Option<String> = 'panel: loop {
            let event = panel.next_event().await;
            let action = match event {
                PanelEvent::Closed => return Ok(()),
                PanelEvent::Resize(w) => {
                    width = w;
                    height = panel_height(panel.height()).or(height);
                    Action::Redraw
                }
                PanelEvent::Key(key) => view.key(&key, &inv),
            };
            match action {
                Action::Close => {
                    let _ = panel.close(Value::Null).await;
                    return Ok(());
                }
                Action::Redraw => {}
                Action::Run(Op::Configure { id }) => break 'panel Some(id),
                Action::Run(op) => {
                    let started = Instant::now();
                    view.status = Some((Tone::Busy, format!("{}…", op.label())));
                    panel.set_lines(view.render(&inv, width, height))?;
                    let (env2, op2) = (env.clone(), op.clone());
                    let mut work = tokio::task::spawn_blocking(move || execute(&env2, &op2));
                    let mut tick = tokio::time::interval(TICK);
                    let result = loop {
                        tokio::select! {
                            done = &mut work => break done.map_err(anyhow::Error::from).and_then(|r| r),
                            _ = tick.tick() => {
                                view.frame += 1;
                                let seconds = started.elapsed().as_secs();
                                let text = if seconds >= 2 { format!("{}… {seconds}s", op.label()) } else { format!("{}…", op.label()) };
                                view.status = Some((Tone::Busy, text));
                                panel.set_lines(view.render(&inv, width, height))?;
                            }
                            event = panel.next_event() => match event {
                                PanelEvent::Closed => {
                                    // pi closed the panel; the operation still finishes.
                                    let text = match work.await.map_err(anyhow::Error::from).and_then(|r| r) {
                                        Ok(text) => host.notify_info(text).await,
                                        Err(err) => host.notify_error(format!("{err:#}")).await,
                                    };
                                    return text;
                                }
                                PanelEvent::Resize(w) => width = w,
                                PanelEvent::Key(_) => {}
                            },
                        }
                    };
                    view.status = Some(match result {
                        Ok(text) => (tone_of(&text), text),
                        Err(err) => (Tone::Warn, format!("{err:#}")),
                    });
                    inv = blocking(env.clone(), |env| Ok(Inventory::read(&env))).await?;
                    let succeeded = view
                        .status
                        .as_ref()
                        .is_some_and(|(tone, _)| *tone == Tone::Info);
                    if let Some(id) = op.configures().filter(|_| succeeded) {
                        if has_open_options(&env, id).await {
                            break 'panel Some(id.to_owned());
                        }
                    }
                }
            }
            let len = view.entries(&inv).len();
            view.selected = view.selected.min(len.saturating_sub(1));
            panel.set_lines(view.render(&inv, width, height))?;
        };
        let _ = panel.close(Value::Null).await;
        if let Some(id) = configure {
            let all = view.status.is_none() || !matches!(view.status, Some((Tone::Info, _)));
            view.status = Some(
                match super::options::prompt_options(&host, &env, &id, all).await {
                    Ok(Some(text)) => (Tone::Info, text),
                    Ok(None) => (Tone::Info, format!("{id} has nothing to configure.")),
                    Err(err) => (Tone::Warn, format!("{err:#}")),
                },
            );
            inv = blocking(env.clone(), |env| Ok(Inventory::read(&env))).await?;
        }
    }
}

/// Whether the plugin has options without a stored value.
async fn has_open_options(env: &Env, id: &str) -> bool {
    let key = id.to_owned();
    blocking(env.clone(), move |env| {
        let (_, options, stored) = install::user_options(&env, &key, None)?;
        Ok(options.iter().any(|opt| !stored.contains_key(&opt.key)))
    })
    .await
    .unwrap_or(false)
}
