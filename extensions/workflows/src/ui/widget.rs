//! The widget above the editor while runs are active: each run's step pipeline and what it
//! does now.

use std::time::Duration;

use pi_ext_sdk::style;

use pi_ext_sdk::serde_json::Value;
use serde::Deserialize;

use crate::model::format_duration;
use crate::runs::{RunState, RunStatus, StepStatus};
use crate::ui::theme::Paint;

pub const KEY: &str = "workflows";
const MAX_RUNS: usize = 3;

pub fn lines(active: &[RunState], paint: &Paint, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for run in active.iter().take(MAX_RUNS) {
        let head = format!(
            "{} {}",
            paint.run_glyph(run.status),
            pi_ext_sdk::style::bold(&run.workflow.name)
        );
        let stats = paint.muted(&run.stats());
        let room =
            width.saturating_sub(style::visible_width(&head) + style::visible_width(&stats) + 4);
        lines.push(style::truncate(
            &format!("{head}  {}  {stats}", pipeline(run, paint, room)),
            width,
        ));
        lines.push(style::truncate(
            &format!("  {}", paint.muted(&now_line(run))),
            width,
        ));
    }
    if active.len() > MAX_RUNS {
        lines.push(paint.dim(&format!(
            "  +{} more · /workflows runs",
            active.len() - MAX_RUNS
        )));
    }
    lines
}

/// `✓ implement › ◐ review › ○ ship`, or `◐ 2/3 review` when that does not fit.
pub fn pipeline(run: &RunState, paint: &Paint, room: usize) -> String {
    let parts: Vec<String> = run
        .workflow
        .steps
        .iter()
        .zip(&run.steps)
        .map(|(step, state)| {
            let name = if state.status == StepStatus::Running {
                style::bold(&step.name)
            } else {
                step.name.clone()
            };
            format!("{} {name}", paint.step_glyph(state.status))
        })
        .collect();
    let full = parts.join(&paint.dim(" › "));
    if style::visible_width(&full) <= room {
        return full;
    }
    match run.current_step() {
        Some(i) => format!(
            "{} {}/{} {}",
            paint.step_glyph(run.steps[i].status),
            i + 1,
            run.steps.len(),
            run.workflow.steps[i].name
        ),
        None => String::new(),
    }
}

/// What the run does now: the wait, or the current step's activity.
pub fn now_line(run: &RunState) -> String {
    if !run.status.active() {
        return String::new();
    }
    if let Some(left) = run.wait_left() {
        return format!(
            "◷ {} · {}",
            format_duration(Duration::from_secs(left.as_secs())),
            run.note
        );
    }
    let running = run
        .steps
        .iter()
        .position(|s| s.status == StepStatus::Running);
    match running {
        Some(i) if run.status == RunStatus::Running => {
            let activity = &run.steps[i].activity;
            let what = if activity.is_empty() {
                "working…"
            } else {
                activity.as_str()
            };
            format!("▸ {}: {what}", run.workflow.steps[i].name)
        }
        _ => run.note.clone(),
    }
}

#[derive(Deserialize)]
struct Report {
    workflow: String,
    status: RunStatus,
    stats: String,
    id: String,
    steps: Vec<ReportStep>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct ReportStep {
    name: String,
    status: StepStatus,
    summary: String,
}

/// The chat card of a finished run, from the details of its `workflow-run` message.
pub fn report_card(details: &Value, paint: &Paint, width: usize) -> Option<Vec<String>> {
    let report = Report::deserialize(details).ok()?;
    let word = match report.status {
        RunStatus::Done => "finished",
        RunStatus::Cancelled => "stopped",
        status => status.label(),
    };
    let head = format!(
        "{} {}  {}",
        paint.run_glyph(report.status),
        style::bold(&paint.accent(&report.workflow)),
        paint.muted(&format!("{word} · {} · {}", report.stats, report.id))
    );
    let name_width = report
        .steps
        .iter()
        .map(|s| s.name.chars().count())
        .max()
        .unwrap_or(0)
        .min(24);
    let mut lines = vec![head];
    for step in &report.steps {
        let line = format!(
            "  {} {:<name_width$}  {}",
            paint.step_glyph(step.status),
            step.name,
            paint.muted(&step.summary)
        );
        lines.push(line);
    }
    if let Some(error) = &report.error {
        lines.push(paint.fg("error", &format!("  {error}")));
    }
    Some(
        lines
            .into_iter()
            .map(|l| style::truncate(&l, width))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use pi_ext_sdk::serde_json::json;

    use super::*;

    #[test]
    fn report_card_lists_steps() {
        let details = json!({ "id": "wf-1", "workflow": "ship", "status": "failed", "stats": "1m · $0.20",
            "steps": [{ "name": "build", "status": "done", "summary": "Built." }, { "name": "deploy", "status": "failed", "summary": "" }],
            "error": "deploy is blocked" });
        let lines: Vec<String> = report_card(&details, &Paint::default(), 80)
            .unwrap()
            .iter()
            .map(|l| style::strip(l))
            .collect();
        assert_eq!(lines[0], "✗ ship  failed · 1m · $0.20 · wf-1");
        assert_eq!(lines[1], "  ✓ build   Built.");
        assert_eq!(lines[3], "  deploy is blocked");
        assert!(report_card(&json!({}), &Paint::default(), 80).is_none());
    }
}
