//! `/workflows`: one panel with nested screens (home, editor, picker, run view). Each screen
//! owns the panel while it is open and returns to the screen that opened it.

use std::ops::Range;
use std::time::Duration;

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::style;

use crate::app::App;
use crate::control::Decision;
use crate::ui::theme::Paint;

mod editor;
mod fields;
mod home;
mod picker;
mod run_view;
mod start;
pub mod theme;
pub mod widget;

/// What every screen needs.
pub struct Ui {
    pub app: App,
    pub host: Host,
    pub paint: Paint,
}

/// The open panel: its size, the lines it shows, and whether pi closed it.
pub struct Screen {
    panel: Panel,
    width: usize,
    shown: Vec<String>,
    pub closed: bool,
}

impl Screen {
    async fn open(host: &Host) -> Result<Self> {
        let mut panel = host.panel(["Loading workflows…"]).open().await?;
        // pi mounts the panel later and reports its first render with `ui.resize`; a dialog
        // opened before that would sit under the panel.
        let first = tokio::time::timeout(Duration::from_secs(1), panel.next_event()).await;
        let closed = matches!(first, Ok(PanelEvent::Closed));
        let width = panel.width().unwrap_or(100);
        Ok(Self {
            panel,
            width,
            shown: Vec::new(),
            closed,
        })
    }

    pub fn width(&self) -> usize {
        self.width.max(40)
    }

    /// Rows a screen may use: most of the terminal, at least 14.
    pub fn rows(&self) -> usize {
        self.panel
            .height()
            .map_or(30, |h| h.saturating_sub(10))
            .max(14)
    }

    /// The next key. `None` after a resize, a tick (with `tick`), or the panel closing.
    pub async fn next(&mut self, tick: bool) -> Option<Key> {
        let event = if tick {
            tokio::select! {
                event = self.panel.next_event() => Some(event),
                () = tokio::time::sleep(Duration::from_millis(500)) => None,
            }
        } else {
            Some(self.panel.next_event().await)
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

    pub fn draw(&mut self, lines: Vec<String>) {
        let width = self.width();
        let lines: Vec<String> = lines
            .into_iter()
            .map(|l| style::truncate(&l, width))
            .collect();
        if lines != self.shown && self.panel.set_lines(lines.clone()).is_ok() {
            self.shown = lines;
        }
    }

    async fn close(self) -> Result<()> {
        if !self.closed {
            self.panel.close(Value::Null).await?;
        }
        Ok(())
    }
}

pub async fn command(app: App, args: String, ctx: CommandCtx) -> Result<()> {
    let host = ctx.host().clone();
    let args = args.trim();
    let (verb, rest) = args
        .split_once(' ')
        .map_or((args, ""), |(v, r)| (v, r.trim()));
    app.refresh(&host);
    if verb == "approve" {
        return match approve(&app, rest) {
            Ok(text) => host.notify_info(text).await,
            Err(text) => host.notify_warning(text).await,
        };
    }
    if !host.has_ui() {
        let text = match verb {
            "runs" => crate::tool::runs_text(&app),
            _ => crate::tool::list_text(&app, &host),
        };
        return host.notify_info(text).await;
    }
    let paint = Paint::load(&host).await;
    let ui = Ui { app, host, paint };
    let mut screen = Screen::open(&ui.host).await?;
    match verb {
        "run" => match ui.app.dirs(&ui.host).find(rest) {
            Some(stored) => {
                start::run_dialog(&ui, &mut screen, stored.workflow).await?;
            }
            None => {
                let message = format!("No workflow named {rest:?}.");
                ui.host.notify_warning(message).await?;
            }
        },
        "" => home::run(&ui, &mut screen, home::Tab::Workflows).await?,
        "runs" => home::run(&ui, &mut screen, home::Tab::Runs).await?,
        "new" => editor::run(&ui, &mut screen, editor::Editor::blank()).await?,
        _ => {
            let name = if verb == "edit" { rest } else { args };
            match ui.app.dirs(&ui.host).find(name) {
                Some(stored) => {
                    editor::run(&ui, &mut screen, editor::Editor::from_stored(stored)).await?
                }
                None => {
                    ui.host
                        .notify_warning(format!("No workflow named {name:?}."))
                        .await?;
                    home::run(&ui, &mut screen, home::Tab::Workflows).await?;
                }
            }
        }
    }
    screen.close().await
}

/// `/workflows approve [<run id>] [run | skip | stop]`; without an id, the one run that waits.
/// It works without a UI, for example `pi -p "/workflows approve wf-abc"`.
fn approve(app: &App, args: &str) -> std::result::Result<String, String> {
    let mut words = args.split_whitespace();
    let first = words.next().unwrap_or("");
    let (id, decision) = match Decision::parse(first) {
        Some(decision) if !first.is_empty() => (None, decision),
        _ if first.is_empty() => (None, Decision::Run),
        _ => {
            let decision = words.next().map_or(Some(Decision::Run), Decision::parse);
            (
                Some(first.to_owned()),
                decision.ok_or("The decision is run, skip, or stop.")?,
            )
        }
    };
    let id = match id {
        Some(id) => id,
        None => {
            let waiting: Vec<String> = app
                .runs
                .all()
                .into_iter()
                .filter(|r| r.status.active() && r.approval.is_some())
                .map(|r| r.id)
                .collect();
            match waiting.as_slice() {
                [id] => id.clone(),
                [] => return Err("No workflow run waits for an approval.".into()),
                ids => return Err(format!("Several runs wait; name one: {}", ids.join(", "))),
            }
        }
    };
    app.approve(&id, decision)
}

/// The rows of a list to show so that `selected` is visible.
pub fn window(selected: usize, total: usize, rows: usize) -> Range<usize> {
    if total <= rows {
        return 0..total;
    }
    let start = selected.saturating_sub(rows / 2).min(total - rows);
    start..start + rows
}

/// Word wrap of plain text to `width` columns.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(10);
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty()
                && style::visible_width(&line) + 1 + style::visible_width(word) > width
            {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

/// `provider/claude-sonnet-4-5` without the provider, for narrow columns.
pub fn short_model(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or(model)
}

/// The first line of a text, cut to `max` characters.
pub fn one_line(text: &str, max: usize) -> String {
    let line = text.trim().lines().next().unwrap_or("").trim();
    if line.chars().count() > max {
        line.chars()
            .take(max.saturating_sub(1))
            .chain(['…'])
            .collect()
    } else {
        line.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_keep_the_selection_visible() {
        assert_eq!(window(0, 5, 10), 0..5);
        assert_eq!(window(0, 50, 10), 0..10);
        assert_eq!(window(25, 50, 10), 20..30);
        assert_eq!(window(49, 50, 10), 40..50);
    }

    #[test]
    fn wraps_words() {
        assert_eq!(wrap("one two three four", 10), ["one two", "three four"]);
        assert_eq!(wrap("a\n\nb", 10), ["a", "", "b"]);
        assert_eq!(one_line("  first line\nsecond", 6), "first…");
    }
}
