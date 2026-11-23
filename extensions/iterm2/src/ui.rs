//! `/iterm2`: the panel loop. Each change is saved to the global config at once, and the tab
//! previews the selected color until the panel closes.

use std::path::Path;
use std::sync::Arc;

use pi_ext_sdk::prelude::*;

use crate::features::{self, Config};
use crate::panel::{Action, Edit, Tone, View};
use crate::reporter::Reporter;

/// Rows pi keeps outside a panel, as in `/hooks`.
const HOST_ROWS: usize = 3;

pub async fn iterm2_command(reporter: Arc<Reporter>, ctx: CommandCtx) -> Result<()> {
    let (host, snap) = (ctx.host(), ctx.snapshot());
    let path = features::global_config_path(&host.agent_dir());
    let mut view = View::new(reporter.terminal().iterm2, display_path(&path));
    load(&reporter, host, &mut view).await;
    if !host.has_ui() {
        return host.notify_info(view.summary()).await;
    }
    let mut width = snap.terminal_width.map_or(100, |w| w as usize);
    let mut height = panel_height(snap.terminal_height.map(|h| h as usize));
    let mut panel = host.panel(view.render(width, height)).open().await?;
    width = panel.width().unwrap_or(width);
    show_preview(&reporter, host, &view).await;
    loop {
        match panel.next_event().await {
            PanelEvent::Closed => break,
            PanelEvent::Resize(w) => {
                width = w;
                height = panel_height(panel.height()).or(height);
            }
            PanelEvent::Key(key) => match view.key(&key) {
                Action::Close => {
                    let _ = panel.close(Value::Null).await;
                    break;
                }
                Action::Redraw => {}
                Action::Save(edit) => {
                    view.status = Some(save(&reporter, host, &path, edit).await);
                    load(&reporter, host, &mut view).await;
                    reporter.apply(host, snap).await;
                }
            },
        }
        show_preview(&reporter, host, &view).await;
        panel.set_lines(view.render(width, height))?;
    }
    reporter.restore(host).await;
    Ok(())
}

/// The effective config and the keys the project file sets.
async fn load(reporter: &Reporter, host: &Host, view: &mut View) {
    let value = reporter.config_value(host).await;
    view.set_value(features::as_object(value).unwrap_or_default());
    let project = features::read_config(&features::project_config_path(&host.cwd()));
    view.project_keys = project
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default();
}

async fn save(reporter: &Reporter, host: &Host, path: &Path, edit: Edit) -> (Tone, String) {
    let fallback = features::resolve(None, None, reporter.legacy_value(host).await);
    match features::edit_global(path, fallback, |config| edit.apply(config)) {
        Ok(()) => {
            let value = reporter.config_value(host).await;
            reporter.set_config(Config::from_value(&value));
            (Tone::Info, edit.describe())
        }
        Err(err) => (Tone::Warn, format!("Cannot save {}: {err}", path.display())),
    }
}

async fn show_preview(reporter: &Reporter, host: &Host, view: &View) {
    match view.preview() {
        Some(color) => reporter.preview(host, color).await,
        None => reporter.restore(host).await,
    }
}

fn panel_height(terminal_rows: Option<usize>) -> Option<usize> {
    terminal_rows.map(|rows| rows.saturating_sub(HOST_ROWS).max(1))
}

/// The path with the home directory as `~`.
fn display_path(path: &Path) -> String {
    match std::env::var_os("HOME").and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}
