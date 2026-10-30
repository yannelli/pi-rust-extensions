//! Text for tool updates, the status widget, run details, and the system prompt section.

use pi_ext_sdk::style;

use crate::agent::AgentDef;
use crate::runs::{Run, Status, first_line, format_duration};

const SHOWN_TOOLS: usize = 6;

/// Progress of the runs of one tool call, for `tool.update`.
pub fn progress_text(runs: &[Run]) -> String {
    runs.iter()
        .map(run_progress)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn run_progress(run: &Run) -> String {
    let p = &run.progress;
    let mut lines = vec![format!(
        "{} {} [{}] {} \u{b7} {} turns \u{b7} {} tools \u{b7} {}",
        run.status.glyph(),
        run.label(),
        run.id,
        run.status.as_str(),
        p.turns,
        p.tools.len(),
        format_duration(run.elapsed())
    )];
    let skip = p.tools.len().saturating_sub(SHOWN_TOOLS);
    if skip > 0 {
        lines.push(format!("  \u{2026} {skip} earlier tool calls"));
    }
    for call in &p.tools[skip..] {
        let mark = match (call.done, call.is_error) {
            (false, _) => "\u{2192}",
            (true, false) => "\u{2713}",
            (true, true) => "\u{2717}",
        };
        lines.push(
            format!("  {mark} {} {}", call.name, call.summary)
                .trim_end()
                .to_owned(),
        );
    }
    let text = p.text.trim();
    if !text.is_empty() {
        let tail: Vec<&str> = text.lines().rev().take(3).collect();
        for line in tail.into_iter().rev() {
            lines.push(format!("  \u{2502} {}", first_line(line, 120)));
        }
    }
    lines.join("\n")
}

/// The final tool text for one or more runs.
pub fn final_text(runs: &[Run]) -> String {
    if let [run] = runs {
        return run.result_text();
    }
    runs.iter()
        .enumerate()
        .map(|(i, run)| {
            format!(
                "## [{}] {} ({}, {})\n\n{}",
                i + 1,
                run.label(),
                run.id,
                run.status.as_str(),
                run.result_text()
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Widget lines for running background runs; empty when none run.
pub fn widget_lines(runs: &[Run], width: usize) -> Vec<String> {
    let running: Vec<&Run> = runs
        .iter()
        .filter(|run| run.background && run.status == Status::Running)
        .collect();
    if running.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![style::truncate(
        &style::dim(&format!(
            "subagents \u{b7} {} running \u{b7} /agents runs",
            running.len()
        )),
        width,
    )];
    for run in running {
        let last_tool = run
            .progress
            .tools
            .last()
            .map(|call| call.name.as_str())
            .unwrap_or("starting");
        let line = format!(
            "{} {} {} \u{b7} {} \u{b7} {}",
            run.status.glyph(),
            style::bold(&run.agent),
            first_line(&run.task, 50),
            last_tool,
            format_duration(run.elapsed())
        );
        lines.push(style::truncate(&line, width));
    }
    lines
}

/// Detail lines of one run for the runs panel.
pub fn detail_lines(run: &Run) -> Vec<String> {
    let mut lines = vec![
        format!(
            "{} {} [{}] {}",
            run.status.glyph(),
            run.agent,
            run.id,
            run.status.as_str()
        ),
        format!(
            "model {} \u{b7} {} turns \u{b7} {} tools \u{b7} {} in / {} out \u{b7} {}",
            run.progress
                .model
                .clone()
                .or_else(|| run.model.clone())
                .unwrap_or_else(|| "default".into()),
            run.progress.turns,
            run.progress.tools.len(),
            run.progress.usage.input,
            run.progress.usage.output,
            format_duration(run.elapsed())
        ),
        String::new(),
        "Task:".into(),
    ];
    lines.extend(run.task.lines().map(|l| format!("  {l}")));
    lines.push(String::new());
    lines.push("Tools:".into());
    for call in &run.progress.tools {
        let mark = if call.is_error {
            "\u{2717}"
        } else if call.done {
            "\u{2713}"
        } else {
            "\u{2192}"
        };
        lines.push(format!("  {mark} {} {}", call.name, call.summary));
    }
    lines.push(String::new());
    lines.push("Output:".into());
    lines.extend(run.result_text().lines().map(|l| format!("  {l}")));
    let stderr = run.stderr.trim();
    if run.status == Status::Failed && !stderr.is_empty() {
        lines.push(String::new());
        lines.push("stderr:".into());
        lines.extend(stderr.lines().map(|l| format!("  {l}")));
    }
    lines
}

/// The `subagents` system prompt section.
pub fn prompt_section(agents: &[AgentDef]) -> String {
    let mut text = String::from(
        "## Subagents\n\nThe `subagent` tool runs a task in a separate pi process with its own context. \
Use it for broad searches, independent work items, and second opinions. Give each task the context it needs: \
the subagent does not see this conversation. Pass `tasks` to run independent tasks in parallel and `async: true` \
for background runs that report when they finish.\n\nAvailable agents:\n",
    );
    for agent in agents {
        text.push_str(&format!(
            "- `{}`: {} (tools: {})\n",
            agent.name,
            agent.description.trim(),
            agent.tools_label()
        ));
    }
    text
}
