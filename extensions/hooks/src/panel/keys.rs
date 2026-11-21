//! Keys of the `/hooks` panel: typing filters, as in `/deferred` and `/claude-plugins`.

use pi_ext_sdk::Key;

use super::{
    Action, AddForm, AddStep, DetailAction, Entry, PAGE, Row, Screen, Tab, Tone, TrustChange, View,
};
use crate::manage::EVENTS;

impl View {
    pub fn key(&mut self, key: &Key, rows: &[Row]) -> Action {
        match self.screen.clone() {
            Screen::List => self.list_key(key, rows),
            Screen::Menu { choice } => self.menu_key(key, rows, choice),
            Screen::Confirm { hash } => {
                self.screen = Screen::List;
                if *key == Key::Enter {
                    return Action::Remove(hash);
                }
                self.status = Some((Tone::Info, "Cancelled.".into()));
                Action::Redraw
            }
            Screen::Add(form) => self.add_key(key, form),
        }
    }

    fn list_key(&mut self, key: &Key, rows: &[Row]) -> Action {
        self.status = None;
        let len = self.entries(rows).len();
        match key {
            Key::Escape | Key::Ctrl('c') if !self.query.is_empty() => {
                self.query.clear();
                self.selected = 0;
            }
            Key::Escape | Key::Ctrl('c') => return Action::Close,
            Key::Tab | Key::BackTab => {
                let at = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
                let step = if *key == Key::Tab {
                    1
                } else {
                    Tab::ALL.len() - 1
                };
                self.tab = Tab::ALL[(at + step) % Tab::ALL.len()];
                self.selected = 0;
            }
            Key::Up if len > 0 => self.selected = (self.selected + len - 1) % len,
            Key::Down if len > 0 => self.selected = (self.selected + 1) % len,
            Key::PageUp => self.selected = self.selected.saturating_sub(PAGE),
            Key::PageDown => self.selected = (self.selected + PAGE).min(len.saturating_sub(1)),
            Key::Home => self.selected = 0,
            Key::End => self.selected = len.saturating_sub(1),
            Key::Left | Key::Right => return self.change(rows, *key == Key::Right),
            Key::Enter => self.open(rows),
            Key::Backspace => {
                self.query.pop();
                self.selected = 0;
            }
            Key::Char(c) => {
                self.query.push(*c);
                self.selected = 0;
            }
            Key::Text(text) => {
                self.query.push_str(text);
                self.selected = 0;
            }
            _ => {}
        }
        Action::Redraw
    }

    /// Enter on a list row: the actions pane, or the add form.
    fn open(&mut self, rows: &[Row]) {
        match self.selected_entry(rows) {
            Some(Entry::Add) => self.screen = Screen::Add(self.add_form()),
            Some(Entry::Hook(_)) => self.screen = Screen::Menu { choice: 0 },
            Some(Entry::Setting(setting)) => {
                let choice = setting.current(&self.options);
                self.screen = Screen::Menu { choice };
            }
            None => {}
        }
    }

    /// ← and →: turn a hook on or off, or step a setting to its previous or next value.
    fn change(&mut self, rows: &[Row], forward: bool) -> Action {
        match self.selected_entry(rows) {
            Some(Entry::Hook(row)) if row.managed() => {
                self.status = Some((Tone::Warn, managed_text(row)));
                Action::Redraw
            }
            Some(Entry::Hook(row)) => Action::Toggle(row.def.hash.clone()),
            Some(Entry::Setting(setting)) => {
                let count = setting.values().len();
                let at = setting.current(&self.options);
                let next = if forward {
                    (at + 1) % count
                } else {
                    (at + count - 1) % count
                };
                Action::Set(setting, next)
            }
            _ => Action::Redraw,
        }
    }

    fn menu_key(&mut self, key: &Key, rows: &[Row], choice: usize) -> Action {
        let count = self.choice_count(rows).max(1);
        match key {
            Key::Up => {
                self.screen = Screen::Menu {
                    choice: (choice + count - 1) % count,
                };
            }
            Key::Down => {
                self.screen = Screen::Menu {
                    choice: (choice + 1) % count,
                };
            }
            Key::Enter => {
                self.screen = Screen::List;
                return self.run_choice(rows, choice);
            }
            _ if key.is_cancel() || matches!(key, Key::Left | Key::Backspace) => {
                self.screen = Screen::List;
            }
            _ => {}
        }
        Action::Redraw
    }

    fn run_choice(&mut self, rows: &[Row], choice: usize) -> Action {
        let row = match self.selected_entry(rows) {
            Some(Entry::Setting(setting)) if choice < setting.values().len() => {
                return Action::Set(setting, choice);
            }
            Some(Entry::Hook(row)) => row,
            _ => return Action::Redraw,
        };
        let hash = row.def.hash.clone();
        let change = match row.actions().get(choice) {
            Some(DetailAction::Toggle) => return Action::Toggle(hash),
            Some(DetailAction::Remove) => {
                self.screen = Screen::Confirm { hash };
                return Action::Redraw;
            }
            Some(DetailAction::TrustAlways) => TrustChange::Always,
            Some(DetailAction::AllowOnce) => TrustChange::Once,
            Some(DetailAction::Untrust) => TrustChange::Untrust,
            Some(DetailAction::Deny) => TrustChange::Deny,
            None => return Action::Redraw,
        };
        Action::Trust(hash, change)
    }

    fn add_key(&mut self, key: &Key, mut form: AddForm) -> Action {
        let field = match form.step {
            AddStep::Event => None,
            AddStep::Matcher => Some(&mut form.matcher),
            AddStep::Command => Some(&mut form.command),
        };
        match (key, field) {
            (Key::Escape | Key::Ctrl('c'), _) => {
                self.screen = Screen::List;
                return Action::Redraw;
            }
            (Key::Tab | Key::BackTab, _) => {
                let scopes = self.scopes();
                let at = scopes.iter().position(|s| *s == form.scope).unwrap_or(0);
                form.scope = scopes[(at + 1) % scopes.len()];
            }
            (Key::Up, None) => form.event = (form.event + EVENTS.len() - 1) % EVENTS.len(),
            (Key::Down, None) => form.event = (form.event + 1) % EVENTS.len(),
            (Key::Enter, _) => match form.step {
                AddStep::Event => form.step = AddStep::Matcher,
                AddStep::Matcher => form.step = AddStep::Command,
                AddStep::Command if form.command.trim().is_empty() => {}
                AddStep::Command => {
                    self.screen = Screen::List;
                    return Action::Add {
                        scope: form.scope,
                        event: EVENTS[form.event].to_owned(),
                        matcher: form.matcher.trim().to_owned(),
                        command: form.command.trim().to_owned(),
                    };
                }
            },
            (Key::Backspace, Some(text)) if text.is_empty() => {
                form.step = match form.step {
                    AddStep::Command => AddStep::Matcher,
                    _ => AddStep::Event,
                };
            }
            (Key::Backspace, Some(text)) => {
                text.pop();
            }
            (Key::Char(c), Some(text)) => text.push(*c),
            (Key::Text(t), Some(text)) => text.push_str(t),
            _ => {}
        }
        self.screen = Screen::Add(form);
        Action::Redraw
    }

    /// The key hints of the current screen.
    pub(super) fn hints(&self) -> Vec<&'static str> {
        match &self.screen {
            Screen::List => {
                let change = if self.tab == Tab::Settings {
                    "←→ change"
                } else {
                    "←→ on/off"
                };
                let esc = if self.query.is_empty() {
                    "esc close"
                } else {
                    "esc clear search"
                };
                let hints = ["↑↓ move", "tab switch", "enter actions", change];
                [&hints[..], &["type to search", esc]].concat()
            }
            Screen::Menu { .. } => vec!["↑↓ choose", "enter run", "esc back"],
            Screen::Confirm { .. } => vec!["enter remove", "any other key cancels"],
            Screen::Add(form) => {
                let step = match form.step {
                    AddStep::Event => vec!["↑↓ event", "enter next"],
                    AddStep::Matcher => vec!["type a matcher", "enter next"],
                    AddStep::Command => vec!["type a command", "enter add"],
                };
                let scope: &[&str] = if self.project_trusted {
                    &["tab scope"]
                } else {
                    &[]
                };
                [&step[..], scope, &["esc cancel"]].concat()
            }
        }
    }
}

pub fn managed_text(row: &Row) -> String {
    format!(
        "{} {} is a managed hook from {}. Change it in that policy file.",
        row.def.event,
        row.matcher(),
        row.def.file.display()
    )
}
