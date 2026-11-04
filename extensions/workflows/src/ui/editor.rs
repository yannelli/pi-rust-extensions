//! The workflow editor: the step tree on the left and the form of the selected row on the right
//! (stacked on narrow terminals).

use std::path::PathBuf;

use pi_ext_sdk::InputOptions;
use pi_ext_sdk::prelude::*;
use pi_ext_sdk::style;

use crate::model::{Step, Task, Workflow, slug};
use crate::store::{Source, Stored};
use crate::ui::fields::{self, Edit, Field, Node};
use crate::ui::home::row_line;
use crate::ui::theme::hints;
use crate::ui::{Screen, Ui, one_line, run_view, short_model, start, window, wrap};

pub struct Editor {
    workflow: Workflow,
    source: Source,
    /// The file the workflow came from; a rename removes it after the save.
    path: Option<PathBuf>,
    dirty: bool,
    in_form: bool,
    row: usize,
    field: usize,
    /// A result line: `(is_error, text)`.
    message: Option<(bool, String)>,
}

impl Editor {
    fn new(workflow: Workflow, source: Source, path: Option<PathBuf>, dirty: bool) -> Self {
        Self {
            workflow,
            source,
            path,
            dirty,
            in_form: false,
            row: 0,
            field: 0,
            message: None,
        }
    }

    pub fn blank() -> Self {
        let step = Step {
            name: "step-1".into(),
            tasks: vec![Task::default()],
            ..Step::default()
        };
        let mut editor = Self::new(
            Workflow {
                steps: vec![step],
                ..Workflow::default()
            },
            Source::Global,
            None,
            true,
        );
        editor.in_form = true;
        editor
    }

    pub fn from_stored(stored: Stored) -> Self {
        Self::new(stored.workflow, stored.source, Some(stored.path), false)
    }

    pub fn copy_of(stored: &Stored) -> Self {
        let mut workflow = stored.workflow.clone();
        workflow.name = format!("{}-copy", workflow.name);
        Self::new(workflow, stored.source, None, true)
    }

    pub fn draft(workflow: Workflow) -> Self {
        Self::new(workflow, Source::Global, None, true)
    }

    fn nodes(&self) -> Vec<Node> {
        let mut nodes = vec![Node::Settings];
        for (i, step) in self.workflow.steps.iter().enumerate() {
            nodes.push(Node::Step(i));
            nodes.extend((0..step.tasks.len()).map(|t| Node::Task(i, t)));
        }
        nodes.push(Node::AddStep);
        nodes
    }

    fn node(&self) -> Node {
        let nodes = self.nodes();
        nodes[self.row.min(nodes.len() - 1)]
    }

    fn select(&mut self, node: Node) {
        if let Some(row) = self.nodes().iter().position(|n| *n == node) {
            self.row = row;
        }
    }

    fn step_of(&self) -> Option<usize> {
        match self.node() {
            Node::Step(i) | Node::Task(i, _) => Some(i),
            _ => None,
        }
    }
}

pub async fn run(ui: &Ui, screen: &mut Screen, mut editor: Editor) -> Result<()> {
    loop {
        screen.draw(editor.render(ui, screen.width(), screen.rows()));
        let Some(key) = screen.next(false).await else {
            if screen.closed {
                return Ok(());
            }
            continue;
        };
        editor.message = None;
        let leave = if editor.in_form {
            editor.form_key(ui, screen, &key).await?
        } else {
            editor.tree_key(ui, screen, &key).await?
        };
        if leave || screen.closed {
            return Ok(());
        }
    }
}

impl Editor {
    /// `true` leaves the editor.
    async fn tree_key(&mut self, ui: &Ui, screen: &mut Screen, key: &Key) -> Result<bool> {
        let last = self.nodes().len() - 1;
        match key {
            Key::Char('K') => self.shift(false),
            Key::Char('J') => self.shift(true),
            k if k.is("up") || k.is("k") => self.row = self.row.saturating_sub(1),
            k if k.is("down") || k.is("j") => self.row = (self.row + 1).min(last),
            k if k.is("enter") || k.is("right") || k.is("tab") || k.is("l") => {
                if self.node() == Node::AddStep {
                    self.add_step(ui).await?;
                } else {
                    self.in_form = true;
                    self.field = 0;
                }
            }
            k if k.is("a") => self.add_step(ui).await?,
            k if k.is("t") => self.add_task(ui).await?,
            k if k.is("d") || k.is("delete") => self.delete(ui).await?,
            k if k.is("c") => self.duplicate(),
            k if k.is("s") || k.is("ctrl+s") => {
                self.save(ui).await?;
            }
            k if k.is("r") => self.run_it(ui, screen).await?,
            k if k.is_cancel() => return self.leave(ui).await,
            _ => {}
        }
        Ok(false)
    }

    async fn form_key(&mut self, ui: &Ui, screen: &mut Screen, key: &Key) -> Result<bool> {
        let node = self.node();
        let fields = fields::fields(node, &self.workflow);
        if fields.is_empty() {
            self.in_form = false;
            return Ok(false);
        }
        self.field = self.field.min(fields.len() - 1);
        let field = fields[self.field];
        match key {
            k if k.is("up") || k.is("k") => self.field = self.field.saturating_sub(1),
            k if k.is("down") || k.is("j") => self.field = (self.field + 1).min(fields.len() - 1),
            k if k.is("enter") || k.is("space") => {
                match fields::edit(field, node, &mut self.workflow, ui, screen).await? {
                    Edit::Changed => self.dirty = true,
                    Edit::Invalid(message) => self.message = Some((true, message)),
                    Edit::Unchanged => {}
                }
            }
            k if k.is("x") || k.is("backspace") || k.is("delete") => {
                if fields::reset(field, node, &mut self.workflow) {
                    self.dirty = true;
                }
            }
            k if k.is("s") || k.is("ctrl+s") => {
                self.save(ui).await?;
            }
            k if k.is("r") => self.run_it(ui, screen).await?,
            k if k.is("ctrl+c") => return self.leave(ui).await,
            k if k.is_cancel() || k.is("left") || k.is("h") || k.is("tab") => self.in_form = false,
            _ => {}
        }
        Ok(false)
    }

    async fn add_step(&mut self, ui: &Ui) -> Result<()> {
        let options = InputOptions {
            placeholder: Some("review".into()),
            ..InputOptions::default()
        };
        let Some(name) = ui.host.input_with("Name of the new step", options).await? else {
            return Ok(());
        };
        let name = name.trim().to_owned();
        if name.is_empty() {
            return Ok(());
        }
        let mut task = Task::default();
        fields::long_text(
            ui,
            &format!("First task of {name} ({{{{name}}}} becomes an input)"),
            &mut task.prompt,
        )
        .await?;
        let at = match self.node() {
            Node::Settings => 0,
            Node::Step(i) | Node::Task(i, _) => i + 1,
            Node::AddStep => self.workflow.steps.len(),
        };
        self.workflow.steps.insert(
            at,
            Step {
                name,
                tasks: vec![task],
                ..Step::default()
            },
        );
        self.dirty = true;
        self.select(Node::Step(at));
        Ok(())
    }

    async fn add_task(&mut self, ui: &Ui) -> Result<()> {
        let Some(step) = self.step_of() else {
            self.message = Some((
                true,
                "Select a step, then press t to add a task to it.".into(),
            ));
            return Ok(());
        };
        let mut task = Task::default();
        let title = format!("New task of {}", self.workflow.steps[step].name);
        if !fields::long_text(ui, &title, &mut task.prompt).await? {
            return Ok(());
        }
        let at = match self.node() {
            Node::Task(_, t) => t + 1,
            _ => self.workflow.steps[step].tasks.len(),
        };
        self.workflow.steps[step].tasks.insert(at, task);
        self.dirty = true;
        self.select(Node::Task(step, at));
        Ok(())
    }

    async fn delete(&mut self, ui: &Ui) -> Result<()> {
        match self.node() {
            Node::Step(i) => {
                let step = &self.workflow.steps[i];
                let question = format!("Delete step {}?", step.name);
                if ui
                    .host
                    .confirm(&question, &format!("It has {} task(s).", step.tasks.len()))
                    .await?
                {
                    self.workflow.steps.remove(i);
                    self.dirty = true;
                }
            }
            Node::Task(s, _) if self.workflow.steps[s].tasks.len() == 1 => {
                self.message = Some((true, "A step needs a task. Delete the step instead.".into()));
            }
            Node::Task(s, t) => {
                self.workflow.steps[s].tasks.remove(t);
                self.dirty = true;
            }
            _ => {}
        }
        self.row = self.row.min(self.nodes().len() - 1);
        Ok(())
    }

    fn shift(&mut self, down: bool) {
        let target = |i: usize, len: usize| {
            if down {
                (i + 1 < len).then_some(i + 1)
            } else {
                i.checked_sub(1)
            }
        };
        match self.node() {
            Node::Step(i) => {
                if let Some(j) = target(i, self.workflow.steps.len()) {
                    self.workflow.steps.swap(i, j);
                    self.dirty = true;
                    self.select(Node::Step(j));
                }
            }
            Node::Task(s, t) => {
                if let Some(j) = target(t, self.workflow.steps[s].tasks.len()) {
                    self.workflow.steps[s].tasks.swap(t, j);
                    self.dirty = true;
                    self.select(Node::Task(s, j));
                }
            }
            _ => {}
        }
    }

    fn duplicate(&mut self) {
        match self.node() {
            Node::Step(i) => {
                let mut step = self.workflow.steps[i].clone();
                step.name = format!("{}-copy", step.name);
                self.workflow.steps.insert(i + 1, step);
                self.select(Node::Step(i + 1));
            }
            Node::Task(s, t) => {
                let task = self.workflow.steps[s].tasks[t].clone();
                self.workflow.steps[s].tasks.insert(t + 1, task);
                self.select(Node::Task(s, t + 1));
            }
            _ => return,
        }
        self.dirty = true;
    }

    /// `true` when the workflow is saved.
    async fn save(&mut self, ui: &Ui) -> Result<bool> {
        if let Some(problem) = self.workflow.problems().first() {
            self.message = Some((true, format!("Fix this first: {problem}")));
            return Ok(false);
        }
        let dirs = ui.app.dirs(&ui.host);
        if self.path.is_none() && dirs.project.is_some() {
            let options = [
                "Global · every project",
                "Project · .pi folder of this project",
            ];
            let Some(choice) = ui.host.select("Save the workflow to", &options).await? else {
                return Ok(false);
            };
            self.source = if choice.starts_with("Project") {
                Source::Project
            } else {
                Source::Global
            };
        }
        let name = slug(&self.workflow.name);
        let target = dirs
            .dir(self.source)
            .map(|dir| dir.join(format!("{name}.json")));
        if let Some(target) = target.filter(|t| t.exists() && Some(t) != self.path.as_ref()) {
            let question = format!("Replace the workflow {name}?");
            if !ui
                .host
                .confirm(&question, &target.display().to_string())
                .await?
            {
                return Ok(false);
            }
        }
        match dirs.save(&self.workflow, self.source) {
            Ok(path) => {
                if let Some(old) = self.path.take().filter(|old| *old != path) {
                    let _ = std::fs::remove_file(old);
                }
                self.message = Some((false, format!("Saved {}", path.display())));
                self.path = Some(path);
                self.workflow.name = name;
                self.dirty = false;
                Ok(true)
            }
            Err(err) => {
                self.message = Some((true, err));
                Ok(false)
            }
        }
    }

    async fn run_it(&mut self, ui: &Ui, screen: &mut Screen) -> Result<()> {
        if self.dirty && !self.save(ui).await? {
            return Ok(());
        }
        if let Some(id) = start::run_dialog(ui, screen, self.workflow.clone()).await? {
            run_view::run(ui, screen, &id).await?;
        }
        Ok(())
    }

    async fn leave(&mut self, ui: &Ui) -> Result<bool> {
        if !self.dirty {
            return Ok(true);
        }
        let options = ["Save and close", "Close without saving", "Keep editing"];
        match ui
            .host
            .select("The workflow has unsaved changes", &options)
            .await?
            .as_deref()
        {
            Some("Save and close") => self.save(ui).await,
            Some("Close without saving") => Ok(true),
            _ => Ok(false),
        }
    }

    fn render(&self, ui: &Ui, width: usize, rows: usize) -> Vec<String> {
        let p = &ui.paint;
        let name = if self.workflow.name.is_empty() {
            "new workflow"
        } else {
            &self.workflow.name
        };
        let state = if self.dirty {
            p.fg("warning", "● unsaved")
        } else {
            p.muted(&format!("saved · {}", self.source.label()))
        };
        let mut lines = vec![p.header(&format!("❖ {name}"), &state, width), p.rule(width)];
        let body = rows.saturating_sub(6).max(8);
        if width >= 96 {
            let left = (width * 2 / 5).clamp(32, 56);
            let right = width - left - 3;
            let tree = self.tree_lines(ui, left, body);
            let form = self.form_lines(ui, right);
            for i in 0..tree.len().max(form.len()).min(body) {
                let l = style::fit(tree.get(i).map_or("", String::as_str), left);
                let r = form.get(i).cloned().unwrap_or_default();
                lines.push(format!("{l}{}{r}", p.dim(" │ ")));
            }
        } else {
            lines.extend(self.tree_lines(ui, width, (body / 2).max(6)));
            lines.push(p.rule(width));
            lines.extend(self.form_lines(ui, width).into_iter().take(body / 2 + 4));
        }
        lines.push(p.rule(width));
        lines.push(self.status_line(ui, width));
        let keys: &[(&str, &str)] = if self.in_form {
            &[
                ("↑↓", "field"),
                ("enter", "change"),
                ("x", "reset"),
                ("esc", "steps"),
                ("s", "save"),
                ("r", "run"),
            ]
        } else {
            &[
                ("↑↓", "move"),
                ("enter", "edit"),
                ("a", "add step"),
                ("t", "add task"),
                ("J/K", "reorder"),
                ("c", "copy"),
                ("d", "delete"),
                ("s", "save"),
                ("r", "run"),
                ("esc", "back"),
            ]
        };
        lines.push(hints(p, keys, width));
        lines
    }

    fn status_line(&self, ui: &Ui, width: usize) -> String {
        let p = &ui.paint;
        let text = match &self.message {
            Some((true, text)) => p.fg("error", &format!(" ✗ {text}")),
            Some((false, text)) => p.fg("success", &format!(" ✓ {text}")),
            None => match self.workflow.problems().as_slice() {
                [] => {
                    let inputs = self.workflow.inputs();
                    let inputs = if inputs.is_empty() {
                        String::new()
                    } else {
                        format!(" · inputs: {}", inputs.join(", "))
                    };
                    p.fg("success", &format!(" ✓ ready to run{inputs}"))
                }
                [only] => p.fg("warning", &format!(" ⚠ {only}")),
                [first, rest @ ..] => {
                    p.fg("warning", &format!(" ⚠ {first} (+{} more)", rest.len()))
                }
            },
        };
        style::truncate(&text, width)
    }

    fn tree_lines(&self, ui: &Ui, width: usize, rows: usize) -> Vec<String> {
        let p = &ui.paint;
        let nodes = self.nodes();
        let row = self.row.min(nodes.len() - 1);
        let mut lines = Vec::new();
        for i in window(row, nodes.len(), rows) {
            let (text, meta) = self.tree_row(ui, nodes[i]);
            let inner = width.saturating_sub(2);
            let meta = p.muted(&meta);
            let room = inner.saturating_sub(style::visible_width(&meta) + 1);
            let text = style::truncate(&text, room);
            let gap =
                inner.saturating_sub(style::visible_width(&text) + style::visible_width(&meta));
            let line = format!("{text}{}{meta}", " ".repeat(gap));
            lines.push(match (i == row, self.in_form) {
                (true, false) => row_line(ui, &line, true, width),
                (true, true) => format!("{} {line}", p.accent("›")),
                _ => format!("  {line}"),
            });
        }
        lines
    }

    fn tree_row(&self, ui: &Ui, node: Node) -> (String, String) {
        let p = &ui.paint;
        let wf = &self.workflow;
        match node {
            Node::Settings => {
                let model = wf
                    .agent
                    .model
                    .as_deref()
                    .map(short_model)
                    .unwrap_or("session model");
                (
                    style::bold("≡ Workflow"),
                    format!("{} · {} · {model}", wf.mode.label(), wf.context.label()),
                )
            }
            Node::Step(i) => {
                let step = &wf.steps[i];
                let scope = &step.agent;
                let mut meta: Vec<&str> = Vec::new();
                meta.extend(scope.model.as_deref().map(short_model));
                meta.extend(scope.thinking.as_deref());
                let meta = if meta.is_empty() {
                    "inherit".to_owned()
                } else {
                    meta.join(" · ")
                };
                (
                    format!(
                        "{} {}",
                        p.accent(&format!("{:>2}", i + 1)),
                        style::bold(&step.name)
                    ),
                    meta,
                )
            }
            Node::Task(s, t) => {
                let task = &wf.steps[s].tasks[t];
                let letter = char::from(b'a' + (t % 26) as u8);
                let mut badges: Vec<String> = Vec::new();
                if let Some(skill) = &task.skill {
                    badges.push(p.accent(&format!("/{skill}")));
                }
                if let Some(delay) = &task.delay {
                    badges.push(p.fg("warning", &format!("◷{delay}")));
                }
                if task.until.is_some() {
                    badges.push(p.fg("warning", "↻goal"));
                } else if task.max_runs() > 1 {
                    let every = task
                        .every
                        .as_deref()
                        .map(|e| format!("/{e}"))
                        .unwrap_or_default();
                    badges.push(p.fg("warning", &format!("×{}{every}", task.max_runs())));
                }
                if task.confirm {
                    badges.push(p.fg("error", "ask"));
                }
                if task.optional {
                    badges.push(p.dim("opt"));
                }
                let prompt = if task.prompt.trim().is_empty() {
                    p.fg("warning", "(empty prompt)")
                } else {
                    p.muted(&one_line(&task.prompt, 200))
                };
                let badges = if badges.is_empty() {
                    String::new()
                } else {
                    format!("{} ", badges.join(" "))
                };
                (
                    format!("   {} {badges}{prompt}", p.dim(&letter.to_string())),
                    String::new(),
                )
            }
            Node::AddStep => (p.accent("+ Add step"), String::new()),
        }
    }

    fn form_lines(&self, ui: &Ui, width: usize) -> Vec<String> {
        let p = &ui.paint;
        let wf = &self.workflow;
        let node = self.node();
        let title = match node {
            Node::Settings => "Workflow settings".to_owned(),
            Node::Step(i) => format!("Step {} · {}", i + 1, wf.steps[i].name),
            Node::Task(s, t) => format!("Task {} of {}", t + 1, wf.steps[s].name),
            Node::AddStep => "Add a step".to_owned(),
        };
        let mut lines = vec![style::bold(&title)];
        let fields = fields::fields(node, wf);
        if fields.is_empty() {
            lines.push(p.muted("Press enter to add a step at the end."));
            return lines;
        }
        let selected = self.field.min(fields.len() - 1);
        for (i, field) in fields.iter().enumerate() {
            if node == Node::Settings && *field == Field::Model {
                lines.push(p.dim("Defaults for every step"));
            }
            if *field == Field::PlannerRules {
                lines.push(p.dim("Planner"));
            }
            let value = fields::value(*field, node, wf, p);
            let line = format!("{:<14}{value}", fields::label(*field));
            lines.push(if self.in_form && i == selected {
                p.selected(&format!("{} {line}", p.accent("❯")), width)
            } else {
                format!("  {line}")
            });
        }
        lines.push(String::new());
        let help = if self.in_form {
            fields::help(fields[selected])
        } else {
            "Press enter to edit this row."
        };
        lines.extend(
            wrap(help, width.saturating_sub(2))
                .iter()
                .map(|l| p.dim(&format!("  {l}"))),
        );
        if let Node::Task(s, t) = node {
            let prompt = &wf.steps[s].tasks[t].prompt;
            if prompt.lines().count() > 1 || prompt.chars().count() > width.saturating_sub(18) {
                lines.push(String::new());
                lines.extend(
                    wrap(prompt, width.saturating_sub(4))
                        .iter()
                        .take(8)
                        .map(|l| format!("  {}", p.muted(l))),
                );
            }
        }
        lines
    }
}
