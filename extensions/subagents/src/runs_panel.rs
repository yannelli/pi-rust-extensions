//! `/agents runs`: a panel with running and recent runs, their output, and cancel.

use std::time::Duration;

use pi_ext_sdk::prelude::*;

use crate::app::App;
use crate::render::detail_lines;
use crate::runs::{Run, Status, first_line, format_duration};

const DETAIL_HEIGHT: usize = 30;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunsView {
    pub selected: usize,
    pub detail: Option<String>,
    pub scroll: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Redraw,
    Close,
    Cancel(String),
}

impl RunsView {
    pub fn key(&mut self, key: &Key, runs: &[Run]) -> Action {
        if self.detail.is_some() {
            return self.detail_key(key, runs);
        }
        if key.is_cancel() {
            return Action::Close;
        }
        let last = runs.len().saturating_sub(1);
        if key.is("up") || key.is("k") {
            self.selected = self.selected.saturating_sub(1);
        } else if key.is("down") || key.is("j") {
            self.selected = (self.selected + 1).min(last);
        } else if key.is("enter") || key.is("right") {
            if let Some(run) = runs.get(self.selected) {
                self.detail = Some(run.id.clone());
                self.scroll = 0;
            }
        } else if key.is("c") || key.is("x") {
            if let Some(run) = runs
                .get(self.selected)
                .filter(|r| r.status == Status::Running)
            {
                return Action::Cancel(run.id.clone());
            }
        }
        Action::Redraw
    }

    fn detail_key(&mut self, key: &Key, runs: &[Run]) -> Action {
        let id = self.detail.clone().unwrap_or_default();
        let total = runs
            .iter()
            .find(|r| r.id == id)
            .map_or(0, |r| detail_lines(r).len());
        let max_scroll = total.saturating_sub(DETAIL_HEIGHT);
        if key.is("escape") || key.is("left") || key.is("backspace") || key.is("q") {
            self.detail = None;
        } else if key.is("ctrl+c") {
            return Action::Close;
        } else if key.is("up") || key.is("k") {
            self.scroll = self.scroll.saturating_sub(1);
        } else if key.is("down") || key.is("j") {
            self.scroll = (self.scroll + 1).min(max_scroll);
        } else if key.is("pageup") {
            self.scroll = self.scroll.saturating_sub(DETAIL_HEIGHT);
        } else if key.is("pagedown") || key.is("space") {
            self.scroll = (self.scroll + DETAIL_HEIGHT).min(max_scroll);
        } else if key.is("end") || key.is("g") {
            self.scroll = max_scroll;
        } else if key.is("c") || key.is("x") {
            return Action::Cancel(id);
        }
        Action::Redraw
    }

    pub fn render(&self, runs: &[Run], width: usize) -> Vec<String> {
        let fit = |line: String| style::truncate(&line, width);
        if let Some(run) = self
            .detail
            .as_ref()
            .and_then(|id| runs.iter().find(|r| &r.id == id))
        {
            let mut lines = vec![
                fit(style::bold("Subagent run")),
                fit(style::dim(
                    "esc back \u{b7} \u{2191}\u{2193} scroll \u{b7} pgup/pgdn \u{b7} c cancel",
                )),
            ];
            let body = detail_lines(run);
            let end = (self.scroll + DETAIL_HEIGHT).min(body.len());
            lines.extend(body[self.scroll.min(end)..end].iter().cloned().map(fit));
            if end < body.len() {
                lines.push(fit(style::dim(&format!(
                    "\u{2026} {} more lines",
                    body.len() - end
                ))));
            }
            return lines;
        }
        let mut lines = vec![
            fit(style::bold("Subagent runs")),
            fit(style::dim(
                "\u{2191}\u{2193} select \u{b7} enter output \u{b7} c cancel \u{b7} q close",
            )),
        ];
        if runs.is_empty() {
            lines.push(fit("  No runs yet.".into()));
        }
        for (i, run) in runs.iter().enumerate() {
            let pointer = if i == self.selected { "\u{276f}" } else { " " };
            let mode = if run.background { "bg" } else { "fg" };
            let line = format!(
                "{pointer} {} {} {mode} {:<9} {:>6}  {}",
                run.status.glyph(),
                run.id,
                run.status.as_str(),
                format_duration(run.elapsed()),
                first_line(&run.label(), 80)
            );
            lines.push(fit(if i == self.selected {
                style::bold(&line)
            } else {
                line
            }));
        }
        lines
    }
}

/// Newest runs first.
fn ordered(app: &App) -> Vec<Run> {
    let mut runs = app.runs.all();
    runs.reverse();
    runs
}

pub async fn open(app: App, host: Host) -> Result<()> {
    let mut view = RunsView::default();
    let width = |panel: &Panel| panel.width().unwrap_or(100);
    let mut panel = host.panel(view.render(&ordered(&app), 100)).open().await?;
    loop {
        let event = tokio::select! {
            event = panel.next_event() => Some(event),
            _ = tokio::time::sleep(Duration::from_millis(500)) => None,
        };
        let runs = ordered(&app);
        match event {
            Some(PanelEvent::Closed) => return Ok(()),
            Some(PanelEvent::Key(key)) => match view.key(&key, &runs) {
                Action::Close => {
                    panel.close(Value::Null).await?;
                    return Ok(());
                }
                Action::Cancel(id) => {
                    app.runs.cancel(&id);
                }
                Action::Redraw => {}
            },
            Some(PanelEvent::Resize(_)) | None => {}
        }
        panel.set_lines(view.render(&ordered(&app), width(&panel)))?;
    }
}
