//! One run: its steps with time, cost, and model, what it does now, and the log, report, and
//! live agent output of the selected step.

use std::time::Duration;

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::style;

use crate::child::{Entry, EntryKind};
use crate::control::Decision;
use crate::model::format_duration;
use crate::runs::{self, RunState, RunStatus};
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
    // Until the user picks a step, the view follows the run's current step, and while the
    // selected step's agent works, it keeps the newest output in sight.
    let mut picked = false;
    let mut follow = true;
    loop {
        let Some(run) = ui.app.runs.get(id) else {
            return Ok(());
        };
        let last = run.steps.len().saturating_sub(1);
        if !picked && let Some(current) = run.current_step() {
            selected = current;
        }
        selected = selected.min(last);
        let output = ui
            .app
            .runs
            .dir(id)
            .map(|dir| runs::read_output(&runs::output_file(&dir, selected)))
            .unwrap_or_default();
        if follow && run.agent_step() == Some(selected) {
            scroll = usize::MAX;
        }
        let (lines, max_scroll) = render(
            ui,
            &run,
            &output,
            selected,
            scroll,
            screen.width(),
            screen.rows(),
        );
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
            (scroll, follow, picked) = (0, true, true);
        } else if key.is("down") || key.is("j") {
            selected = (selected + 1).min(last);
            (scroll, follow, picked) = (0, true, true);
        } else if key.is("pageup") {
            scroll = scroll.saturating_sub(10);
            follow = false;
        } else if key.is("pagedown") || key.is("space") {
            scroll = (scroll + 10).min(max_scroll);
            follow = scroll == max_scroll;
        } else if key.is("end") {
            (scroll, follow) = (max_scroll, true);
        } else {
            act(ui, &key, &run).await?;
        }
    }
}

/// The run keys, shared with the runs tab: stop (`x`), skip the wait (`s`), resume (`r`), switch
/// background and foreground (`b`), answer an approval (`a` run it, `n` skip the task), and send
/// the working agent a message (`m`).
pub async fn act(ui: &Ui, key: &Key, run: &RunState) -> Result<()> {
    let result = if key.is("m") && run.agent_step().is_some() {
        let title = "Message to the agent (it reads it after its current tool call)";
        let Some(message) = ui.host.input(title, None).await? else {
            return Ok(());
        };
        if message.trim().is_empty() {
            return Ok(());
        }
        ui.app.steer(&run.id, &message)
    } else if key.is("x") && run.status.active() {
        let question = format!("Stop the run of {}?", run.workflow.name);
        let detail = "You can resume it later at the task that did not finish.";
        if !ui.host.confirm(&question, detail).await? {
            return Ok(());
        }
        ui.app.stop(&run.id)
    } else if key.is("s") {
        ui.app.skip_wait(&run.id)
    } else if key.is("r") {
        ui.app.resume(&ui.host, &run.id).await
    } else if key.is("b") {
        ui.app.set_background(&run.id, !run.background)
    } else if key.is("a") || key.is("n") {
        let decision = if key.is("a") {
            Decision::Run
        } else {
            Decision::Skip
        };
        ui.app.approve(&run.id, decision).map(drop)
    } else {
        return Ok(());
    };
    if let Err(err) = result {
        ui.host.notify_warning(err).await?;
    }
    Ok(())
}

/// The keys of `act` that apply to the run now.
pub fn run_keys(run: &RunState) -> Vec<(&'static str, &'static str)> {
    let mut keys = Vec::new();
    if run.approval.is_some() {
        keys.extend([("a", "approve"), ("n", "skip task")]);
    }
    if run.agent_step().is_some() {
        keys.push(("m", "message agent"));
    }
    if run.status.active() {
        keys.push(("x", "stop"));
        if run.wait_until_ms.is_some() {
            keys.push(("s", "skip wait"));
        }
        keys.push((
            "b",
            if run.background {
                "foreground"
            } else {
                "background"
            },
        ));
    } else if run.status != RunStatus::Done {
        keys.push(("r", "resume"));
    }
    keys
}

fn render(
    ui: &Ui,
    run: &RunState,
    output: &[Entry],
    selected: usize,
    scroll: usize,
    width: usize,
    rows: usize,
) -> (Vec<String>, usize) {
    let p = &ui.paint;
    let mode = match (run.status.active(), run.background) {
        (true, true) => " · background",
        (true, false) => " · foreground",
        (false, _) => "",
    };
    let right = format!(
        "{} {}{mode} · {}",
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
    if let Some(approval) = run.approval.as_ref().filter(|_| run.status.active()) {
        let ask = format!(" ? \"{}\" waits for your approval", approval.task);
        lines.push(p.fg("warning", &ask));
    } else {
        let now = widget::now_line(run);
        if run.status.active() && !now.is_empty() {
            lines.push(p.accent(&format!(" {now}")));
        }
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
        if !output.is_empty() || !state.writing.is_empty() {
            detail.push(String::new());
            detail.push(p.dim(" Output"));
            let text_width = width.saturating_sub(4);
            for entry in output {
                detail.extend(output_lines(ui, entry, text_width));
            }
            if !state.writing.is_empty() {
                let mut writing = wrap(&state.writing, text_width);
                if let Some(last) = writing.last_mut() {
                    last.push('▍');
                }
                detail.extend(writing.iter().map(|l| format!("  {l}")));
            }
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
    let mut keys: Vec<(&str, &str)> = vec![("↑↓", "step"), ("pgup/pgdn/end", "scroll")];
    keys.extend(run_keys(run));
    keys.push(("esc", "back"));
    lines.push(hints(p, &keys, width));
    (lines, max_scroll)
}

/// How many lines of a prompt or a tool error the output shows.
const SHORT_LINES: usize = 3;

fn output_lines(ui: &Ui, entry: &Entry, width: usize) -> Vec<String> {
    let p = &ui.paint;
    let short = |lines: Vec<String>| -> Vec<String> {
        let lines: Vec<String> = lines.into_iter().filter(|l| !l.trim().is_empty()).collect();
        let more = lines.len() > SHORT_LINES;
        let mut lines: Vec<String> = lines.into_iter().take(SHORT_LINES).collect();
        if more && let Some(last) = lines.last_mut() {
            last.push_str(" …");
        }
        lines
    };
    match entry.kind {
        EntryKind::Prompt => short(wrap(&entry.text, width.saturating_sub(2)))
            .iter()
            .map(|l| p.dim(&format!("  › {l}")))
            .collect(),
        EntryKind::User => wrap(&entry.text, width.saturating_sub(4))
            .iter()
            .enumerate()
            .map(|(i, l)| p.accent(&format!("  {} {l}", if i == 0 { "›" } else { " " })))
            .collect(),
        EntryKind::Assistant => wrap(&entry.text, width)
            .iter()
            .map(|l| format!("  {l}"))
            .collect(),
        EntryKind::Tool => vec![p.muted(&format!(
            "  ⏵ {}",
            crate::ui::one_line(&entry.text, width.saturating_sub(2))
        ))],
        EntryKind::Error => short(wrap(&entry.text, width.saturating_sub(2)))
            .iter()
            .map(|l| p.fg("error", &format!("  ✗ {l}")))
            .collect(),
    }
}
