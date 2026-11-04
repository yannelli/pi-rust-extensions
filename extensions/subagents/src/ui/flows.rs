//! Dialogs inside the panel (text, choice, picker) and the flows that use them: new, copy,
//! rename, delete or reset, move, and the save-or-discard question.

use std::path::PathBuf;

use pi_ext_sdk::Key;

use crate::agent::Source;
use crate::registry::{Layers, sanitize_name};
use crate::ui::draft::{Draft, Field, TEMPLATES, Template, Values, template};
use crate::ui::line::LineEdit;
use crate::ui::ops;
use crate::ui::picker::{Picked, Picker};
use crate::ui::state::{Effect, Focus, State};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum After {
    Close,
    List,
    Runs,
}

#[derive(Debug, Clone)]
pub enum Pending {
    Save(Option<After>),
    Create { name: String, values: Values },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextThen {
    Field(Field),
    /// `None` copies the selected agent.
    NewName(Option<Template>),
    Rename,
}

#[derive(Debug, Clone)]
pub enum ChoiceThen {
    Template,
    Scope(Pending),
    Leave(After),
    /// The files each option removes; `reset` for built-ins.
    Remove {
        name: String,
        sets: Vec<Vec<PathBuf>>,
        reset: bool,
    },
    Move(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickThen {
    Model,
    Tools,
    Disallowed,
}

#[derive(Debug, Clone)]
pub struct Text {
    pub title: String,
    pub help: String,
    pub edit: LineEdit,
    pub then: TextThen,
}

#[derive(Debug, Clone)]
pub struct Choice {
    pub title: String,
    /// `(label, note)`.
    pub options: Vec<(String, String)>,
    pub row: usize,
    pub then: ChoiceThen,
}

#[derive(Debug, Clone)]
pub enum Modal {
    Text(Text),
    Choice(Choice),
    Pick(Picker, PickThen),
}

impl Modal {
    pub fn text(title: &str, help: &str, text: &str, then: TextThen) -> Self {
        Modal::Text(Text {
            title: title.to_owned(),
            help: help.to_owned(),
            edit: LineEdit::new(text),
            then,
        })
    }

    fn choice(title: &str, options: Vec<(String, String)>, then: ChoiceThen) -> Self {
        Modal::Choice(Choice {
            title: title.to_owned(),
            options,
            row: 0,
            then,
        })
    }

    pub fn leave(after: After) -> Self {
        let options = [
            ("Save", "write the changes, then continue"),
            ("Discard", "drop the changes"),
            ("Keep editing", ""),
        ];
        let options = options.map(|(l, n)| (l.to_owned(), n.to_owned())).to_vec();
        Self::choice(
            "The agent has unsaved changes",
            options,
            ChoiceThen::Leave(after),
        )
    }

    pub fn scope(layers: &Layers, title: &str, pending: Pending) -> Self {
        let mut options = vec![(
            "Global".to_owned(),
            format!("every project · {}", layers.global.display()),
        )];
        if let Some(project) = &layers.project {
            options.push((
                "Project".into(),
                format!("this project only · {}", project.display()),
            ));
        }
        Self::choice(title, options, ChoiceThen::Scope(pending))
    }
}

fn pair(label: impl Into<String>, note: impl Into<String>) -> (String, String) {
    (label.into(), note.into())
}

impl State {
    pub fn modal_key(&mut self, modal: Modal, key: &Key) -> Effect {
        match modal {
            Modal::Text(mut text) => match key {
                Key::Escape | Key::Ctrl('c') => Effect::None,
                Key::Enter => self.text_done(text),
                other => {
                    text.edit.key(other);
                    self.modal = Some(Modal::Text(text));
                    Effect::None
                }
            },
            Modal::Choice(mut choice) => {
                let last = choice.options.len().saturating_sub(1);
                let pick = match key {
                    Key::Char(c @ '1'..='9') => {
                        Some(*c as usize - '1' as usize).filter(|i| *i <= last)
                    }
                    Key::Enter => Some(choice.row),
                    _ => None,
                };
                match key {
                    _ if pick.is_some() => return self.choice_done(choice.then, pick.unwrap_or(0)),
                    Key::Up | Key::Char('k') => choice.row = choice.row.saturating_sub(1),
                    Key::Down | Key::Char('j') => choice.row = (choice.row + 1).min(last),
                    k if k.is_cancel() => return Effect::None,
                    _ => {}
                }
                self.modal = Some(Modal::Choice(choice));
                Effect::None
            }
            Modal::Pick(mut picker, then) => {
                let picked = picker.key(key);
                match picked {
                    Picked::Pending => self.modal = Some(Modal::Pick(picker, then)),
                    Picked::Cancel => {}
                    picked => self.picked(then, picked),
                }
                Effect::None
            }
        }
    }

    fn picked(&mut self, then: PickThen, picked: Picked) {
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        let v = &mut draft.values;
        match (then, picked) {
            (PickThen::Model, Picked::One(model)) => {
                v.model = Some(model).filter(|m| m != "inherit");
            }
            (PickThen::Tools, Picked::Many(tools)) => v.tools = tools,
            (PickThen::Disallowed, Picked::Many(tools)) => v.disallowed = tools.unwrap_or_default(),
            _ => {}
        }
    }

    fn text_done(&mut self, text: Text) -> Effect {
        let value = text.edit.text.trim().to_owned();
        match text.then {
            TextThen::Field(field) => {
                if let Some(draft) = self.draft.as_mut() {
                    match field {
                        Field::Description => draft.values.description = value,
                        _ => draft.values.max_turns = value,
                    }
                    let problems = draft.problems(&[]);
                    let problem = problems.into_iter().find(|p| p.field == field && p.error);
                    if let Some(problem) = problem {
                        self.err(problem.text);
                    }
                }
            }
            TextThen::NewName(kind) => match ops::check_name(&value, &self.entries) {
                Err(err) => {
                    self.err(err);
                    self.modal = Some(Modal::Text(text));
                }
                Ok(name) => {
                    let values = match kind {
                        Some(kind) => template(kind, &name),
                        None => match self.selected() {
                            Some(e) => Values::of(&e.def),
                            None => return Effect::None,
                        },
                    };
                    self.ask_scope(name, values);
                }
            },
            TextThen::Rename => {
                let Some(agent) = self.selected().map(|e| e.def.clone()) else {
                    return Effect::None;
                };
                if sanitize_name(&value) == agent.name {
                    return Effect::None;
                }
                match ops::check_name(&value, &self.entries) {
                    Err(err) => {
                        self.err(err);
                        self.modal = Some(Modal::Text(text));
                    }
                    Ok(name) => {
                        self.done(ops::rename(&agent, &name), true);
                    }
                }
            }
        }
        Effect::None
    }

    fn ask_scope(&mut self, name: String, values: Values) {
        if self.setup.layers.project.is_some() {
            let title = format!("Create {name} in");
            let pending = Pending::Create { name, values };
            self.modal = Some(Modal::scope(&self.setup.layers, &title, pending));
        } else {
            let dir = self.setup.layers.global.clone();
            self.create(&dir, &name, &values);
        }
    }

    fn create(&mut self, dir: &std::path::Path, name: &str, values: &Values) {
        if self.done(ops::create(dir, name, values), true) {
            self.tab = crate::ui::state::Tab::Agents;
            self.focus = Focus::Form;
            self.field = 1;
        }
    }

    fn choice_done(&mut self, then: ChoiceThen, row: usize) -> Effect {
        match then {
            ChoiceThen::Template if row < TEMPLATES.len() => {
                let (kind, label, _) = TEMPLATES[row];
                let title = format!("Name of the new agent ({label})");
                let help = "Lowercase letters, digits, - and _.";
                self.modal = Some(Modal::text(&title, help, "", TextThen::NewName(Some(kind))));
            }
            ChoiceThen::Template => self.start_copy(),
            ChoiceThen::Scope(pending) => {
                let source = if row == 0 {
                    Source::Global
                } else {
                    Source::Project
                };
                let dir = self.scope_dir(source);
                match pending {
                    Pending::Save(after) => {
                        if self.save_to(&dir)
                            && let Some(after) = after
                        {
                            return self.after(after);
                        }
                    }
                    Pending::Create { name, values } => self.create(&dir, &name, &values),
                }
            }
            ChoiceThen::Leave(after) => match row {
                0 if self.save(Some(after)) => return self.after(after),
                1 => {
                    self.draft = self.selected().map(|e| Draft::new(&e.def));
                    return self.after(after);
                }
                _ => {}
            },
            ChoiceThen::Remove { name, sets, reset } => {
                if let Some(files) = sets.get(row) {
                    self.done(ops::delete(&name, files, reset), true);
                }
            }
            ChoiceThen::Move(dir) => {
                if let (0, Some(agent)) = (row, self.selected().map(|e| e.def.clone())) {
                    self.done(ops::move_to(&agent, &dir), false);
                }
            }
        }
        Effect::None
    }

    pub fn scope_dir(&self, source: Source) -> PathBuf {
        let dir = ops::scope_dir(&self.setup.layers, source);
        dir.unwrap_or_else(|| self.setup.layers.global.clone())
    }

    pub fn start_new(&mut self) {
        let mut options: Vec<(String, String)> = TEMPLATES
            .iter()
            .map(|(_, label, note)| pair(*label, *note))
            .collect();
        if let Some(e) = self.selected() {
            options.push(pair(
                format!("Copy of {}", e.def.name),
                "every field of the selected agent",
            ));
        }
        self.modal = Some(Modal::choice(
            "New agent: start from",
            options,
            ChoiceThen::Template,
        ));
    }

    pub fn start_copy(&mut self) {
        let Some(name) = self.selected().map(|e| e.def.name.clone()) else {
            return;
        };
        let title = format!("Name of the copy of {name}");
        let help = "The copy gets every field. Lowercase letters, digits, - and _.";
        let prefill = format!("{name}-copy");
        self.modal = Some(Modal::text(&title, help, &prefill, TextThen::NewName(None)));
    }

    pub fn start_rename(&mut self) {
        let Some(e) = self.selected().cloned() else {
            return;
        };
        if e.def.builtin {
            return self.err(format!(
                "{} is built-in, so its name is fixed. Copy it (c) instead.",
                e.def.name
            ));
        }
        if let Some(plugin) = &e.plugin {
            return self.err(format!(
                "The {plugin} plugin owns {}. Copy it (c) instead.",
                e.def.name
            ));
        }
        let title = format!("Rename {}", e.def.name);
        let help = "Renames each file of the agent. Lowercase letters, digits, - and _.";
        self.modal = Some(Modal::text(&title, help, &e.def.name, TextThen::Rename));
    }

    pub fn start_delete(&mut self) {
        let Some(e) = self.selected().cloned() else {
            return;
        };
        let (name, files, reset) = (e.def.name.clone(), e.def.files.clone(), e.def.builtin);
        if let Some(plugin) = &e.plugin {
            return self.err(format!(
                "The {plugin} plugin owns {name}. Remove it with /claude-plugins."
            ));
        }
        if files.is_empty() {
            return self.err(format!("{name} is built-in and has no override to reset."));
        }
        let show = |f: &PathBuf| f.display().to_string();
        let mut options = Vec::new();
        let mut sets = Vec::new();
        if let [only] = files.as_slice() {
            let label = if reset {
                "Reset to the built-in"
            } else {
                "Delete"
            };
            options.push(pair(label, format!("removes {}", show(only))));
            sets.push(files.clone());
        } else {
            let top = files[files.len() - 1].clone();
            let scope = ops::scope_of(&self.setup.layers, &top).label();
            options.push(pair(format!("Remove the {scope} file"), show(&top)));
            sets.push(vec![top]);
            let all: Vec<String> = files.iter().map(show).collect();
            options.push(pair(
                format!("Remove all {} files", files.len()),
                all.join(", "),
            ));
            sets.push(files);
        }
        options.push(pair("Cancel", ""));
        let title = format!("{} {name}?", if reset { "Reset" } else { "Delete" });
        self.modal = Some(Modal::choice(
            &title,
            options,
            ChoiceThen::Remove { name, sets, reset },
        ));
    }

    pub fn start_move(&mut self) {
        let Some(e) = self.selected().cloned() else {
            return;
        };
        let name = &e.def.name;
        if let Some(plugin) = &e.plugin {
            return self.err(format!(
                "The {plugin} plugin owns {name}. Copy it (c) instead."
            ));
        }
        let Some(top) = e.top() else {
            return self.err(format!("{name} has no file yet. Save a change first."));
        };
        if self.setup.layers.project.is_none() {
            return self.err("The project is not trusted, so agents can only be global.");
        }
        let from = ops::scope_of(&self.setup.layers, top);
        let to = if from == Source::Project {
            Source::Global
        } else {
            Source::Project
        };
        let dir = self.scope_dir(to);
        let options = vec![
            pair(format!("Move to {}", to.label()), dir.display().to_string()),
            pair("Cancel", ""),
        ];
        let title = format!("Move the {} file of {name}", from.label());
        self.modal = Some(Modal::choice(&title, options, ChoiceThen::Move(dir)));
    }
}
