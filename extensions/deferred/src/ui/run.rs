//! Opens the `/deferred` panel and applies its edits.

use std::time::Instant;

use pi_ext_sdk::list::{ListOverflow, ListScroll};
use pi_ext_sdk::prelude::*;

use super::rows::{self, Inventory, Sources};
use super::{Action, Tone, View};
use crate::config::{self, NAMESPACE};
use crate::{Shared, commands, lock, project_trusted};

/// Rows pi keeps outside a panel: the cwd and usage footer lines, and the top row of
/// fullscreen mode.
const HOST_ROWS: usize = 3;

fn panel_height(terminal_rows: Option<usize>) -> Option<usize> {
    terminal_rows.map(|rows| rows.saturating_sub(HOST_ROWS).max(1))
}

/// Every tool, skill, context file, and section, from pi when it answers and else from the
/// last prompt.
async fn inventory(state: &Shared, ctx: &CommandCtx) -> Inventory {
    let host = ctx.host();
    let tools = host.all_tools().await.unwrap_or_else(|err| {
        pi_ext_sdk::warn!("tools.getAll failed: {}", err.message());
        Vec::new()
    });
    let live = host
        .pi_call("ctx", "getSystemPromptOptions", Vec::new())
        .await
        .ok();
    let block =
        |path: std::path::PathBuf| config::read_file(&path).and_then(|v| v.get(NAMESPACE).cloned());
    let global = block(host.agent_dir().join("settings.json"));
    let project = block(host.project_dir().join("settings.json"));
    let session = lock(state);
    let options = live
        .filter(Value::is_object)
        .or_else(|| session.options.clone())
        .unwrap_or_else(|| json!({}));
    rows::build(&Sources {
        config: &session.config,
        tools: &tools,
        options: &options,
        context_window: ctx.snapshot().model.as_ref().and_then(|m| m.context_window),
        loaded: &session.loaded,
        global: global.as_ref(),
        project: project.as_ref(),
    })
}

pub(crate) async fn open(state: Shared, ctx: &CommandCtx) -> Result<()> {
    let (host, snap) = (ctx.host(), ctx.snapshot());
    let trusted = project_trusted(host, snap).await;
    let repo = lock(&state).config.repo.clone();
    let mut view = View::new(trusted).with_repo(repo);
    view.overflow = ListOverflow::read(host).await;
    let mut inv = inventory(&state, ctx).await;
    let mut width = snap.terminal_width.map_or(100, |w| w as usize);
    let mut height = panel_height(snap.terminal_height.map(|h| h as usize));
    let mut panel = host.panel(view.render(&inv, width, height)).open().await?;
    width = panel.width().unwrap_or(width);
    view.scroll.reset(Instant::now());
    let mut tick = tokio::time::interval(ListScroll::TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let event = tokio::select! {
            event = panel.next_event() => Some(event),
            _ = tick.tick(), if view.overflow == ListOverflow::Scroll => None,
        };
        let Some(event) = event else {
            let limit = view.scroll_limit(&inv, width);
            if view.scroll.tick(Instant::now(), limit) {
                panel.set_lines(view.render(&inv, width, height))?;
            }
            continue;
        };
        match event {
            PanelEvent::Closed => return Ok(()),
            PanelEvent::Resize(w) => {
                width = w;
                height = panel_height(panel.height()).or(height);
            }
            PanelEvent::Key(key) => match view.key(&key, &inv) {
                Action::Close => {
                    let _ = panel.close(Value::Null).await;
                    return Ok(());
                }
                Action::Redraw => {}
                Action::Write { change, scope } => {
                    view.status = Some((
                        Tone::Busy,
                        format!("Saving the rule for {}…", change.pattern),
                    ));
                    panel.set_lines(view.render(&inv, width, height))?;
                    let result = commands::write_rule(
                        &state,
                        ctx,
                        change.kind,
                        &change.pattern,
                        change.mode,
                        &scope,
                    )
                    .await;
                    view.status = Some(match result {
                        Ok(text) => (Tone::Info, text),
                        Err(text) => (Tone::Warn, text),
                    });
                    inv = inventory(&state, ctx).await;
                }
                Action::Unload(name) => {
                    lock(&state).loaded.remove(&name);
                    let text = format!(
                        "Forgot that {name} was loaded; its rule applies again from the next prompt."
                    );
                    view.status = Some((Tone::Info, text));
                    inv = inventory(&state, ctx).await;
                }
            },
        }
        view.clamp(&inv);
        view.scroll.reset(Instant::now());
        panel.set_lines(view.render(&inv, width, height))?;
    }
}
