//! `/agents`: one panel with the agent list, a form with every field, in-panel pickers and
//! dialogs, and a runs tab. `state` holds the keys; this module runs the panel and pi calls.

use std::time::Duration;

use pi_ext_sdk::prelude::*;

use crate::app::App;
use crate::runs::Run;

mod act;
pub mod draft;
mod flows;
mod form;
pub mod line;
pub mod ops;
pub mod paint;
pub mod picker;
pub mod state;
pub mod view;

pub use flows::{After, Modal};
use paint::Paint;
use picker::Item;
use state::{Effect, Setup, State};

#[cfg(test)]
mod tests;

/// Where the panel opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    Agents(Option<String>),
    Runs,
}

/// The open panel, its size, and the lines it shows.
struct Screen {
    panel: Panel,
    width: usize,
    shown: Vec<String>,
    /// A key that came before pi's first resize.
    early: Option<Key>,
    closed: bool,
}

impl Screen {
    async fn open(host: &Host, lines: Vec<String>) -> Result<Self> {
        let mut panel = host.panel(lines.clone()).open().await?;
        // pi reports the first render with `ui.resize`; a dialog opened before that would sit
        // under the panel, so wait for it briefly.
        let first = tokio::time::timeout(Duration::from_secs(1), panel.next_event()).await;
        let (early, closed) = match first {
            Ok(PanelEvent::Key(key)) => (Some(key), false),
            Ok(PanelEvent::Closed) => (None, true),
            _ => (None, false),
        };
        let width = panel.width().unwrap_or(100);
        Ok(Self {
            panel,
            width,
            shown: lines,
            early,
            closed,
        })
    }

    fn rows(&self) -> usize {
        self.panel
            .height()
            .map_or(32, |h| h.saturating_sub(6))
            .max(16)
    }

    /// The next key; `None` after a resize, a tick, or the panel closing.
    async fn next(&mut self) -> Option<Key> {
        if let Some(key) = self.early.take() {
            return Some(key);
        }
        let event = tokio::select! {
            event = self.panel.next_event() => Some(event),
            () = tokio::time::sleep(Duration::from_millis(500)) => None,
        };
        match event {
            Some(PanelEvent::Key(key)) => Some(key),
            Some(PanelEvent::Resize(width)) => {
                self.width = width;
                None
            }
            Some(PanelEvent::Closed) => {
                self.closed = true;
                None
            }
            None => None,
        }
    }

    fn draw(&mut self, lines: Vec<String>) {
        if lines != self.shown && self.panel.set_lines(lines.clone()).is_ok() {
            self.shown = lines;
        }
    }
}

/// The `provider/id` models pi can use now, with their names as notes.
async fn models(host: &Host) -> Vec<Item> {
    let models = host
        .pi_call("ctx.modelRegistry", "getAvailable", Vec::new())
        .await;
    let models = models.unwrap_or_default();
    let list = models.as_array().into_iter().flatten();
    list.filter_map(|m| {
        let value = format!("{}/{}", m["provider"].as_str()?, m["id"].as_str()?);
        let mut note: Vec<String> = m["name"].as_str().map(str::to_owned).into_iter().collect();
        if let Some(window) = m["contextWindow"].as_u64() {
            note.push(format!("{}k context", window / 1000));
        }
        if m["reasoning"].as_bool() == Some(true) {
            note.push("thinking".into());
        }
        Some(Item::new(value, note.join(" · ")))
    })
    .collect()
}

async fn tools(host: &Host) -> Vec<Item> {
    let tools = host.all_tools().await.unwrap_or_default();
    let tools = tools.iter().filter_map(|t| {
        let name = t["name"].as_str().filter(|n| *n != "subagent")?;
        let note = t["description"].as_str().unwrap_or("");
        Some(Item::new(name, note.lines().next().unwrap_or("")))
    });
    tools.collect()
}

/// Newest runs first.
fn ordered(app: &App) -> Vec<Run> {
    let mut runs = app.runs.all();
    runs.reverse();
    runs
}

pub async fn open(app: App, host: Host, start: Start) -> Result<()> {
    let _ = app.agents(&host);
    let setup = Setup {
        layers: app.layers(&host),
        models: models(&host).await,
        tools: tools(&host).await,
        advertise: app.config(&host).advertise,
        context_entries: crate::context::host_supports(&host),
    };
    let paint = Paint::load(&host).await;
    let mut state = State::new(setup);
    match start {
        Start::Runs => state.open_runs(None),
        Start::Agents(Some(name)) => {
            state.reload(&name);
            if state.selected().is_some_and(|e| e.def.name == name) {
                state.focus = state::Focus::Form;
                state.field = 1;
            } else {
                state.err(format!("No agent named {name:?}."));
            }
        }
        Start::Agents(None) => {}
    }
    let first = view::render(&state, &paint, &ordered(&app), 100, 32);
    let mut screen = Screen::open(&host, first).await?;
    loop {
        let runs = ordered(&app);
        screen.draw(view::render(
            &state,
            &paint,
            &runs,
            screen.width,
            screen.rows(),
        ));
        let Some(key) = screen.next().await else {
            if screen.closed {
                return Ok(());
            }
            continue;
        };
        let runs = ordered(&app);
        match state.key(&key, &runs) {
            Effect::None => {}
            Effect::Close => break,
            Effect::CancelRun(id) => {
                app.runs.cancel(&id);
            }
            Effect::EditPrompt { title, text } => {
                let edited = host.editor(&title, Some(&text)).await?;
                state.prompt_edited(edited);
            }
            Effect::EditFile { path, text } => {
                let title = format!("{} (the agent file)", path.display());
                let edited = host.editor(&title, Some(&text)).await?;
                state.file_edited(&path, edited);
            }
        }
    }
    if !screen.closed {
        screen.panel.close(Value::Null).await?;
    }
    Ok(())
}
