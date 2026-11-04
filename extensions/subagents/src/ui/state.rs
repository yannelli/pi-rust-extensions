//! The state of the `/agents` panel and its keys on the list and the form.
//! File changes run here; `Effect` names what needs pi (the editor, closing, a cancel).

use std::path::PathBuf;

use pi_ext_sdk::Key;

use crate::registry::Layers;
use crate::runs::Run;
use crate::runs_panel::RunsView;
use crate::ui::draft::{Draft, FIELDS, Field};
use crate::ui::flows::{After, Modal, PickThen, TextThen};
use crate::ui::line::LineEdit;
use crate::ui::ops::{self, Done, Entry};
use crate::ui::picker::{Item, Picker, fuzzy};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Agents,
    Runs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Form,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    Close,
    EditPrompt { title: String, text: String },
    EditFile { path: PathBuf, text: String },
    CancelRun(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Error,
    Info,
}

/// What the panel knows about the session when it opens.
#[derive(Debug, Clone)]
pub struct Setup {
    pub layers: Layers,
    pub models: Vec<Item>,
    pub tools: Vec<Item>,
    /// The `advertise` config key.
    pub advertise: bool,
    /// pi lists agents as context entries, so changes need `/reload`.
    pub context_entries: bool,
}

pub struct State {
    pub setup: Setup,
    pub entries: Vec<Entry>,
    pub tab: Tab,
    pub focus: Focus,
    /// The list row; the row after the last agent is "+ New agent".
    pub row: usize,
    pub field: usize,
    pub filter: String,
    pub searching: Option<LineEdit>,
    pub draft: Option<Draft>,
    pub modal: Option<Modal>,
    pub message: Option<(Level, String)>,
    pub runs: RunsView,
    pub runs_agent: Option<String>,
}

impl State {
    pub fn new(setup: Setup) -> Self {
        let entries = ops::entries(&setup.layers);
        let mut state = Self {
            setup,
            entries,
            tab: Tab::Agents,
            focus: Focus::List,
            row: 0,
            field: 0,
            filter: String::new(),
            searching: None,
            draft: None,
            modal: None,
            message: None,
            runs: RunsView::default(),
            runs_agent: None,
        };
        state.sync();
        state
    }

    pub fn known_tools(&self) -> Vec<String> {
        self.setup.tools.iter().map(|t| t.value.clone()).collect()
    }

    /// Entry indexes in list order, best search match first.
    pub fn visible(&self) -> Vec<usize> {
        let needle = self.filter.trim();
        let mut scored: Vec<(i64, usize)> = (0..self.entries.len())
            .filter_map(|i| {
                let e = &self.entries[i];
                let hay = format!("{} {} {}", e.def.name, e.badge(), e.def.description);
                let own = fuzzy(needle, &e.def.name);
                let score = own.or_else(|| fuzzy(needle, &hay).map(|s| s - 5000));
                score.map(|s| (s, i))
            })
            .collect();
        if !needle.is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        scored.into_iter().map(|(_, i)| i).collect()
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.visible().get(self.row).map(|&i| &self.entries[i])
    }

    pub fn dirty(&self) -> bool {
        self.draft.as_ref().is_some_and(Draft::dirty)
    }

    pub fn field(&self) -> Field {
        FIELDS[self.field.min(FIELDS.len() - 1)]
    }

    /// Rebuilds the draft when the selection moved to another agent.
    pub fn sync(&mut self) {
        self.row = self.row.min(self.visible().len());
        let name = self.selected().map(|e| e.def.name.clone());
        if self.draft.as_ref().map(|d| &d.name) != name.as_ref() || !self.dirty() {
            self.draft = self.selected().map(|e| Draft::new(&e.def));
        }
        if self.draft.is_none() {
            self.focus = Focus::List;
        }
    }

    /// Loads the agents again and selects `name`, or the row nearest to the old one.
    pub fn reload(&mut self, name: &str) {
        self.entries = ops::entries(&self.setup.layers);
        let find = |s: &Self| {
            s.visible()
                .iter()
                .position(|&i| s.entries[i].def.name == name)
        };
        if find(self).is_none() {
            self.filter.clear();
        }
        if let Some(row) = find(self) {
            self.row = row;
        }
        self.draft = None;
        self.sync();
    }

    pub fn ok(&mut self, text: impl Into<String>) {
        self.message = Some((Level::Ok, text.into()));
    }

    pub fn err(&mut self, text: impl Into<String>) {
        self.message = Some((Level::Error, text.into()));
    }

    /// Shows the result of a file change and reloads; `true` on success.
    pub fn done(&mut self, done: Done, listed: bool) -> bool {
        match done {
            Ok((text, name)) => {
                let hint = if listed && self.setup.context_entries {
                    " Run /reload so pi lists the change."
                } else {
                    ""
                };
                self.reload(&name);
                self.ok(format!("{text}.{hint}"));
                true
            }
            Err(text) => {
                self.err(text);
                false
            }
        }
    }

    pub fn key(&mut self, key: &Key, runs: &[Run]) -> Effect {
        self.message = None;
        if let Some(modal) = self.modal.take() {
            return self.modal_key(modal, key);
        }
        match (self.tab, self.focus) {
            (Tab::Runs, _) => self.runs_key(key, runs),
            _ if self.searching.is_some() => self.search_key(key),
            (Tab::Agents, Focus::List) => self.list_key(key),
            (Tab::Agents, Focus::Form) => self.form_key(key),
        }
    }

    fn move_row(&mut self, key: &Key, last: usize) -> bool {
        self.row = match key {
            Key::Up | Key::Char('k') => self.row.saturating_sub(1),
            Key::Down | Key::Char('j') => (self.row + 1).min(last),
            Key::Home => 0,
            Key::End => last,
            Key::PageUp => self.row.saturating_sub(10),
            Key::PageDown => (self.row + 10).min(last),
            _ => return false,
        };
        true
    }

    fn list_key(&mut self, key: &Key) -> Effect {
        let last = self.visible().len();
        if self.move_row(key, last) {
            self.sync();
            return Effect::None;
        }
        match key {
            Key::Enter | Key::Right | Key::Char('l') if self.row == last => self.start_new(),
            Key::Enter | Key::Right | Key::Char('l') => {
                self.focus = Focus::Form;
                self.field = 1;
            }
            Key::Char('/') => self.searching = Some(LineEdit::new(&self.filter)),
            Key::Escape if !self.filter.is_empty() => {
                self.filter.clear();
                self.sync();
            }
            Key::Tab | Key::BackTab => self.open_runs(None),
            k if k.is_cancel() => return Effect::Close,
            _ => return self.action_key(key),
        }
        Effect::None
    }

    /// The agent actions of the list and the form; the form must be saved first.
    fn action_key(&mut self, key: &Key) -> Effect {
        let Key::Char(c) = key else {
            return Effect::None;
        };
        if "ncrdmfu".contains(*c) && self.dirty() {
            self.err("Save (s) or discard (esc) the changes first.");
            return Effect::None;
        }
        match c {
            'n' => self.start_new(),
            'c' => self.start_copy(),
            'r' => self.start_rename(),
            'd' => self.start_delete(),
            'm' => self.start_move(),
            'u' => {
                let name = self.selected().map(|e| e.def.name.clone());
                self.open_runs(name);
            }
            'p' | 'e' => return self.edit_prompt(),
            'f' => return self.edit_file(),
            _ => {}
        }
        Effect::None
    }

    fn search_key(&mut self, key: &Key) -> Effect {
        let Some(mut search) = self.searching.take() else {
            return Effect::None;
        };
        match key {
            Key::Enter => {}
            Key::Escape => self.filter.clear(),
            Key::Up | Key::Down => {
                self.move_row(key, self.visible().len());
                self.searching = Some(search);
            }
            other => {
                search.key(other);
                self.filter = search.text.clone();
                self.row = 0;
                self.searching = Some(search);
            }
        }
        self.sync();
        Effect::None
    }

    fn form_key(&mut self, key: &Key) -> Effect {
        let field = self.field();
        let last = FIELDS.len() - 1;
        match key {
            Key::Up | Key::Char('k') => self.field = self.field.saturating_sub(1),
            Key::Down | Key::Char('j') => self.field = (self.field + 1).min(last),
            Key::Home => self.field = 0,
            Key::End => self.field = last,
            Key::Left | Key::Right if field.cycles() => {
                if let Some(draft) = self.draft.as_mut() {
                    draft.cycle(field, *key == Key::Right);
                }
            }
            Key::Enter | Key::Char(' ') => return self.activate(field),
            Key::Char('x') | Key::Backspace | Key::Delete => {
                let cleared = self.draft.as_mut().is_some_and(|d| d.clear(field));
                if !cleared {
                    let label = field.label();
                    self.err(format!("{label} has no default. Press enter to change it."));
                }
            }
            Key::Char('s') | Key::Ctrl('s') => {
                self.save(None);
            }
            Key::Escape | Key::Left | Key::Char('h') => return self.leave(After::List),
            Key::Tab | Key::BackTab => return self.leave(After::Runs),
            k if k.is_cancel() => return self.leave(After::Close),
            _ => return self.action_key(key),
        }
        Effect::None
    }

    fn activate(&mut self, field: Field) -> Effect {
        let Some(draft) = self.draft.as_mut() else {
            return Effect::None;
        };
        let v = &draft.values;
        let title = format!("{} of {}", field.label(), draft.name);
        match field {
            Field::Name => self.start_rename(),
            Field::Description | Field::MaxTurns => {
                let text = match field {
                    Field::Description => &v.description,
                    _ => &v.max_turns,
                };
                let then = TextThen::Field(field);
                self.modal = Some(Modal::text(&title, field.help(), text, then));
            }
            Field::Model => {
                let mut items = vec![Item::new("inherit", "session model and thinking level")];
                items.extend(self.setup.models.iter().cloned());
                let current = v.model.as_deref().unwrap_or("inherit");
                let picker = Picker::one(&title, items, Some(current));
                self.modal = Some(Modal::Pick(picker, PickThen::Model));
            }
            Field::Tools | Field::Disallowed => {
                let tools = Some(self.setup.tools.clone()).filter(|t| !t.is_empty());
                let items = tools.unwrap_or_else(default_tools);
                let (current, all, then) = match field {
                    Field::Tools => (v.tools.clone(), true, PickThen::Tools),
                    _ => (Some(v.disallowed.clone()), false, PickThen::Disallowed),
                };
                let picker = Picker::many(&title, items, current.as_deref(), all);
                self.modal = Some(Modal::Pick(picker, then));
            }
            Field::Prompt => return self.edit_prompt(),
            _ => {
                draft.cycle(field, true);
            }
        }
        Effect::None
    }

    /// Asks to save or discard before `after` when the form has changes.
    pub fn leave(&mut self, after: After) -> Effect {
        if self.dirty() {
            self.modal = Some(Modal::leave(after));
            return Effect::None;
        }
        self.after(after)
    }

    pub fn after(&mut self, after: After) -> Effect {
        match after {
            After::Close => return Effect::Close,
            After::List => self.focus = Focus::List,
            After::Runs => self.open_runs(None),
        }
        Effect::None
    }
}

/// pi's built-in tools, for hosts that do not list their tools.
fn default_tools() -> Vec<Item> {
    let tools = [
        "read", "bash", "edit", "write", "grep", "find", "ls", "todo",
    ];
    tools.into_iter().map(|t| Item::new(t, "")).collect()
}
