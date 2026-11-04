//! One run: its steps with time, cost, and model, what it does now, and the log and report of
//! the selected step.

use std::time::Duration;

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::style;

use crate::model::format_duration;
use crate::runs::{RunState, RunStatus};
use crate::ui::home::row_line;
use crate::ui::theme::hints;
use crate::ui::{Screen, Ui, short_model, widget, window, wrap};

pub async fn run(ui: &Ui, screen: &mut Screen, id: &str) -> Result<()> {
    let mut selected = ui
        .app
        .runs
        .get(id)
        .and_then(|r| r.current_step())
        .unwrap_or(0);
    let mut scroll = 0usize;
    loop {
        let Some(run) = ui.app.runs.get(id) else {
            return Ok(());
        };
        let last = run.steps.len().saturating_sub(1);
        selected = selected.min(last);
        let (lines, max_scroll) = render(ui, &run, selected, scroll, screen.width(), screen.rows());
        scroll = scroll.min(max_scroll);
        screen.draw(lines);
        let Some(key) = screen.next(true).await else {
            if screen.closed {
                return Ok(());
            }
            continue;
        };
        if key.is_cancel() || key.is("left") {
            return Ok(());
        } else if key.is("up") || key.is("k") {
            selected = selected.saturating_sub(1);
            scroll = 0;
        } else if key.is("down") || key.is("j") {
            selected = (selected + 1).min(last);
            scroll = 0;
        } else if key.is("pageup") {
            scroll = scroll.saturating_sub(10);
        } else if key.is("pagedown") || key.is("space") {
            scroll = (scroll + 10).min(max_scroll);
        } else {
            act(ui, &key, &run).await?;
        }
    }
}

/// Stop (`x`), skip the wait (`s`), and resume (`r`), shared with the runs tab.
pub async fn act(ui: &Ui, key: &Key, run: &RunState) -> Result<()> {
    if key.is("x") && run.status.active() {
        let question = format!("Stop the run of {}?", run.workflow.name);
        if ui
            .host
            .confirm(
                &question,
                "You can resume it later at the task that did not finish.",
            )
            .await?
        {
            ui.app.stop(&run.id);
        }
    } else if key.is("s") && !ui.app.skip_wait(&run.id) {
        ui.host.notify_info("The run is not waiting.").await?;
    } else if key.is("r")
        && let Err(err) = ui.app.resume(&ui.host, &run.id).await
    {
        ui.host.notify_warning(err).await?;
    }
    Ok(())
}

fn render(
    ui: &Ui,
    run: &RunState,
    selected: usize,
    scroll: usize,
    width: usize,
    rows: usize,
) -> (Vec<String>, usize) {
    let p = &ui.paint;
    let right = format!(
        "{} {} · {}",
        p.run_glyph(run.status),
        run.status.label(),
        p.muted(&run.stats())
    );
    let mut lines = vec![
        p.header(
            &format!("❖ {} · {}", run.workflow.name, run.id),
            &right,
            width,
        ),
        p.rule(width),
    ];
    let name_width = run
        .workflow
        .steps
        .iter()
        .map(|s| s.name.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(8, 24);
    for i in window(selected, run.steps.len(), (rows / 3).max(4)) {
        let (step, state) = (&run.workflow.steps[i], &run.steps[i]);
        let time = state
            .elapsed()
            .map(|d| format_duration(Duration::from_secs(d.as_secs())))
            .unwrap_or_default();
        let model = state
            .model
            .as_deref()
            .or(step.agent.model.as_deref())
            .map(short_model)
            .unwrap_or("");
        let cost = if state.cost > 0.0 {
            format!("${:.2}", state.cost)
        } else {
            String::new()
        };
        let runs = match state.runs {
            0 => format!("{} task(s)", step.tasks.len()),
            1 => "1 run".to_owned(),
            n => format!("{n} runs"),
        };
        let text = format!(
            "{} {:<name_width$}  {}",
            p.step_glyph(state.status),
            step.name,
            p.muted(&format!("{model:<22} {time:>7} {cost:>7}  {runs}"))
        );
        lines.push(row_line(ui, &text, i == selected, width));
    }
    if run.steps.is_empty() {
        lines.push(p.muted("  The planner has not planned any steps yet."));
    }
    lines.push(p.rule(width));
    let now = widget::now_line(run);
    if run.status.active() && !now.is_empty() {
        lines.push(p.accent(&format!(" {now}")));
    }
    if let Some(error) = &run.error {
        lines.extend(
            wrap(error, width.saturating_sub(4))
                .iter()
                .take(4)
                .map(|l| p.fg("error", &format!(" ✗ {l}"))),
        );
    }
    let mut detail: Vec<String> = Vec::new();
    if let (Some(step), Some(state)) = (run.workflow.steps.get(selected), run.steps.get(selected)) {
        detail.push(style::bold(&format!(
            " {} · {}",
            step.name,
            state.status.label()
        )));
        for entry in &state.log {
            detail.extend(
                wrap(entry, width.saturating_sub(5))
                    .iter()
                    .map(|l| p.muted(&format!("  · {l}"))),
            );
        }
        if !state.summary.is_empty() {
            detail.push(String::new());
            detail.push(p.dim(" Report"));
            detail.extend(
                wrap(&state.summary, width.saturating_sub(4))
                    .iter()
                    .map(|l| format!("  {l}")),
            );
        }
        if detail.len() == 1 {
            detail.push(p.muted("  Nothing yet."));
        }
    }
    let room = rows.saturating_sub(lines.len() + 3).max(5);
    let max_scroll = detail.len().saturating_sub(room);
    let scroll = scroll.min(max_scroll);
    lines.extend(detail.iter().skip(scroll).take(room).cloned());
    if scroll + room < detail.len() {
        lines.push(p.dim(&format!(
            "  ↓ {} more lines · pgdn",
            detail.len() - scroll - room
        )));
    }
    lines.push(p.rule(width));
    let mut keys: Vec<(&str, &str)> = vec![("↑↓", "step"), ("pgup/pgdn", "scroll")];
    if run.status.active() {
        keys.extend([("x", "stop"), ("s", "skip wait")]);
    } else if run.status != RunStatus::Done {
        keys.push(("r", "resume"));
    }
    keys.push(("esc", "back"));
    lines.push(hints(p, &keys, width));
    (lines, max_scroll)
}
