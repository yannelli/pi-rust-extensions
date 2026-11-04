//! The first screen: saved workflows and runs, on two tabs.

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::style;

use crate::model::Mode;
use crate::planner;
use crate::runs::RunState;
use crate::store::Stored;
use crate::ui::editor::{self, Editor};
use crate::ui::theme::hints;
use crate::ui::{Screen, Ui, one_line, run_view, start, widget, window, wrap};

/// List rows stop here on wide terminals, so their right column stays near the names.
const ROW_WIDTH: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Workflows,
    Runs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Workflow(usize),
    New,
    Generate,
}

struct Home {
    tab: Tab,
    row: usize,
    run_row: usize,
}

fn rows(workflows: &[Stored]) -> Vec<Row> {
    let mut rows: Vec<Row> = (0..workflows.len()).map(Row::Workflow).collect();
    rows.extend([Row::New, Row::Generate]);
    rows
}

pub async fn run(ui: &Ui, screen: &mut Screen, tab: Tab) -> Result<()> {
    let mut home = Home {
        tab,
        row: 0,
        run_row: 0,
    };
    loop {
        let (workflows, errors) = ui.app.dirs(&ui.host).load();
        let runs = ui.app.runs.all();
        screen.draw(home.render(
            ui,
            &workflows,
            &errors,
            &runs,
            screen.width(),
            screen.rows(),
        ));
        let tick = home.tab == Tab::Runs || ui.app.runs.any_active();
        let Some(key) = screen.next(tick).await else {
            if screen.closed {
                return Ok(());
            }
            continue;
        };
        if key.is_cancel() {
            return Ok(());
        }
        if key.is("tab") || key.is("shift+tab") || key.is("left") || key.is("right") {
            home.tab = if home.tab == Tab::Workflows {
                Tab::Runs
            } else {
                Tab::Workflows
            };
            continue;
        }
        match home.tab {
            Tab::Workflows => home.workflow_key(ui, screen, &key, &workflows).await?,
            Tab::Runs => home.run_key(ui, screen, &key, &runs).await?,
        }
        if screen.closed {
            return Ok(());
        }
    }
}

impl Home {
    async fn workflow_key(
        &mut self,
        ui: &Ui,
        screen: &mut Screen,
        key: &Key,
        workflows: &[Stored],
    ) -> Result<()> {
        let rows = rows(workflows);
        self.row = self.row.min(rows.len() - 1);
        let selected = match rows[self.row] {
            Row::Workflow(i) => Some(&workflows[i]),
            _ => None,
        };
        if key.is("up") || key.is("k") {
            self.row = self.row.saturating_sub(1);
        } else if key.is("down") || key.is("j") {
            self.row = (self.row + 1).min(rows.len() - 1);
        } else if key.is("n") || (key.is("enter") && rows[self.row] == Row::New) {
            editor::run(ui, screen, Editor::blank()).await?;
        } else if key.is("g") || (key.is("enter") && rows[self.row] == Row::Generate) {
            generate(ui, screen).await?;
        } else if let Some(stored) = selected {
            if key.is("enter") || key.is("e") {
                editor::run(ui, screen, Editor::from_stored(stored.clone())).await?;
            } else if key.is("r") {
                if let Some(id) = start::run_dialog(ui, screen, stored.workflow.clone()).await? {
                    run_view::run(ui, screen, &id).await?;
                }
            } else if key.is("c") {
                editor::run(ui, screen, Editor::copy_of(stored)).await?;
            } else if key.is("d") || key.is("delete") {
                let question = format!("Delete workflow {}?", stored.workflow.name);
                if ui
                    .host
                    .confirm(&question, &stored.path.display().to_string())
                    .await?
                {
                    std::fs::remove_file(&stored.path)?;
                }
            }
        }
        Ok(())
    }

    async fn run_key(
        &mut self,
        ui: &Ui,
        screen: &mut Screen,
        key: &Key,
        runs: &[RunState],
    ) -> Result<()> {
        let Some(run) = runs.get(self.run_row.min(runs.len().saturating_sub(1))) else {
            return Ok(());
        };
        if key.is("up") || key.is("k") {
            self.run_row = self.run_row.saturating_sub(1);
        } else if key.is("down") || key.is("j") {
            self.run_row = (self.run_row + 1).min(runs.len() - 1);
        } else if key.is("enter") {
            run_view::run(ui, screen, &run.id).await?;
        } else {
            run_view::act(ui, key, run).await?;
        }
        Ok(())
    }

    fn render(
        &self,
        ui: &Ui,
        workflows: &[Stored],
        errors: &[String],
        runs: &[RunState],
        width: usize,
        rows: usize,
    ) -> Vec<String> {
        let p = &ui.paint;
        let active = runs.iter().filter(|r| r.status.active()).count();
        let summary = match active {
            0 => format!("{} saved", workflows.len()),
            n => format!("{} saved · {n} running", workflows.len()),
        };
        let tab = |label: &str, on: bool| {
            if on {
                style::bold(&p.accent(&style::underline(label)))
            } else {
                p.muted(label)
            }
        };
        let runs_label = if runs.is_empty() {
            "Runs".to_owned()
        } else {
            format!("Runs ({})", runs.len())
        };
        let mut lines = vec![
            p.header("❖ Workflows", &p.muted(&summary), width),
            format!(
                " {}   {}",
                tab("Workflows", self.tab == Tab::Workflows),
                tab(&runs_label, self.tab == Tab::Runs)
            ),
            p.rule(width),
        ];
        let body = rows.saturating_sub(10).max(4);
        match self.tab {
            Tab::Workflows => self.render_workflows(ui, workflows, errors, width, body, &mut lines),
            Tab::Runs => self.render_runs(ui, runs, width, body, &mut lines),
        }
        lines.push(p.rule(width));
        let keys: &[(&str, &str)] = match self.tab {
            Tab::Workflows => &[
                ("enter", "edit"),
                ("r", "run"),
                ("n", "new"),
                ("g", "generate"),
                ("c", "copy"),
                ("d", "delete"),
                ("tab", "runs"),
                ("q", "close"),
            ],
            Tab::Runs => &[
                ("enter", "open"),
                ("x", "stop"),
                ("s", "skip wait"),
                ("r", "resume"),
                ("tab", "workflows"),
                ("q", "close"),
            ],
        };
        lines.push(hints(p, keys, width));
        lines
    }

    fn render_workflows(
        &self,
        ui: &Ui,
        workflows: &[Stored],
        errors: &[String],
        width: usize,
        body: usize,
        lines: &mut Vec<String>,
    ) {
        let p = &ui.paint;
        let rows = rows(workflows);
        let selected = self.row.min(rows.len() - 1);
        let name_width = workflows
            .iter()
            .map(|s| s.workflow.name.chars().count())
            .max()
            .unwrap_or(0)
            .clamp(12, 28);
        for i in window(selected, rows.len(), body) {
            let text = match rows[i] {
                Row::Workflow(w) => {
                    let s = &workflows[w];
                    let wf = &s.workflow;
                    let steps = match (wf.mode, wf.steps.len()) {
                        (Mode::Dynamic, _) => "dynamic · planned".to_owned(),
                        (_, 1) => "1 step".to_owned(),
                        (_, n) => format!("{n} steps"),
                    };
                    let meta = format!("{steps} · {}", wf.context.label());
                    let left = format!("{:<name_width$}  {}", wf.name, p.muted(&meta));
                    let source = p.dim(s.source.label());
                    let gap = width.min(ROW_WIDTH).saturating_sub(
                        style::visible_width(&left) + style::visible_width(&source) + 4,
                    );
                    format!("{left}{}{source}", " ".repeat(gap))
                }
                Row::New => p.accent("+ New workflow"),
                Row::Generate => p.accent("✦ Generate from a description"),
            };
            lines.push(row_line(ui, &text, i == selected, width.min(ROW_WIDTH)));
        }
        for error in errors {
            lines.push(p.fg(
                "warning",
                &format!("  ⚠ {}", one_line(error, width.saturating_sub(6))),
            ));
        }
        lines.push(p.rule(width));
        match rows[selected] {
            Row::Workflow(w) => lines.extend(preview(ui, &workflows[w], width)),
            Row::New => lines.push(p.muted("  Start from one empty step, and fill it in the editor.")),
            Row::Generate => lines.extend(
                wrap(
                    "Describe the workflow in your own words. The session model drafts it, and you review it in the editor before you save.",
                    width.saturating_sub(4),
                )
                .iter()
                .map(|l| p.muted(&format!("  {l}"))),
            ),
        }
    }

    fn render_runs(
        &self,
        ui: &Ui,
        runs: &[RunState],
        width: usize,
        body: usize,
        lines: &mut Vec<String>,
    ) {
        let p = &ui.paint;
        if runs.is_empty() {
            lines.push(p.muted("  No runs yet. Start one from the Workflows tab with r."));
            return;
        }
        let selected = self.run_row.min(runs.len() - 1);
        for i in window(selected, runs.len(), body) {
            let run = &runs[i];
            let text = format!(
                "{} {:<20} {}  {}",
                p.run_glyph(run.status),
                run.workflow.name,
                p.muted(&format!("{:<11} {:<16}", run.status.label(), run.stats())),
                p.dim(&run.id)
            );
            lines.push(row_line(ui, &text, i == selected, width.min(ROW_WIDTH)));
        }
        lines.push(p.rule(width));
        let run = &runs[selected];
        lines.push(format!(
            "  {}",
            widget::pipeline(run, p, width.saturating_sub(4))
        ));
        let now = widget::now_line(run);
        if !now.is_empty() {
            lines.push(p.muted(&format!("  {now}")));
        }
        if let Some(error) = &run.error {
            lines.extend(
                wrap(error, width.saturating_sub(4))
                    .iter()
                    .take(3)
                    .map(|l| p.fg("error", &format!("  {l}"))),
            );
        }
    }
}

/// A list row with the pointer, highlighted when selected.
pub fn row_line(ui: &Ui, text: &str, selected: bool, width: usize) -> String {
    if selected {
        ui.paint
            .selected(&format!("{} {text}", ui.paint.accent("❯")), width)
    } else {
        format!("  {text}")
    }
}

fn preview(ui: &Ui, stored: &Stored, width: usize) -> Vec<String> {
    let p = &ui.paint;
    let wf = &stored.workflow;
    let mut lines: Vec<String> = Vec::new();
    if !wf.description.is_empty() {
        lines.extend(
            wrap(&wf.description, width.saturating_sub(4))
                .iter()
                .take(3)
                .map(|l| format!("  {l}")),
        );
    }
    let steps: Vec<String> = wf.steps.iter().map(|s| s.name.clone()).collect();
    if !steps.is_empty() {
        let names: Vec<String> = steps.iter().map(|s| p.accent(s)).collect();
        lines.push(format!("  {}", names.join(&p.dim(" › "))));
    }
    let inputs = wf.inputs();
    let tasks = wf.steps.iter().flat_map(|s| &s.tasks);
    let approvals = tasks.clone().filter(|t| t.confirm).count();
    let waits = tasks
        .filter(|t| t.delay.is_some() || t.until.is_some() || t.repeat.is_some())
        .count();
    let mut facts = Vec::new();
    if !inputs.is_empty() {
        facts.push(format!("inputs: {}", inputs.join(", ")));
    }
    if approvals > 0 {
        facts.push(format!("{approvals} approval(s)"));
    }
    if waits > 0 {
        facts.push(format!("{waits} timed task(s)"));
    }
    if !facts.is_empty() {
        lines.push(p.muted(&format!("  {}", facts.join(" · "))));
    }
    lines
}

async fn generate(ui: &Ui, screen: &mut Screen) -> Result<()> {
    let title = "Describe the workflow: the steps, who does each, models, approvals, and waits";
    let Some(description) = ui
        .host
        .editor(title, None)
        .await?
        .filter(|d| !d.trim().is_empty())
    else {
        return Ok(());
    };
    let width = screen.width();
    screen.draw(vec![
        ui.paint.header("❖ Workflows", "", width),
        String::new(),
        ui.paint
            .accent("  ✦ The session model drafts the workflow…"),
    ]);
    let catalog = ui.app.catalog(&ui.host).await;
    let available = planner::available(&ui.host, &catalog).await;
    match planner::generate(&ui.host, &description, available).await {
        Ok(workflow) => editor::run(ui, screen, Editor::draft(workflow)).await,
        Err(err) => {
            ui.host
                .notify_error(format!("The workflow draft failed: {err}"))
                .await
        }
    }
}
