//! Keys of the `/deferred` panel: typing filters, as in `/acp`.

use pi_ext_sdk::{Key, style};

use super::render::muted;
use super::rows::{self, Inventory, Scope};
use super::{Action, Choice, Entry, Screen, TABS, Tone, View, modes};
use crate::config::{Kind, Mode};

const PAGE: usize = 10;

impl View {
    pub fn key(&mut self, key: &Key, inv: &Inventory) -> Action {
        match self.screen.clone() {
            Screen::List => self.list_key(key, inv),
            Screen::Menu { choice } => self.menu_key(key, inv, choice),
            Screen::Scope { change, choice } => match key {
                Key::Up | Key::Down => {
                    let len = self.scopes().len().max(1);
                    let step = if *key == Key::Down { 1 } else { len - 1 };
                    self.screen = Screen::Scope {
                        change,
                        choice: (choice + step) % len,
                    };
                    Action::Redraw
                }
                Key::Enter => {
                    self.screen = Screen::List;
                    let scope = self.scopes().get(choice).cloned();
                    Action::Write {
                        change,
                        scope: scope.unwrap_or(Scope::Global),
                    }
                }
                _ if key.is_cancel() || matches!(key, Key::Left | Key::Backspace) => {
                    self.screen = Screen::List;
                    Action::Redraw
                }
                _ => Action::Redraw,
            },
        }
    }

    fn list_key(&mut self, key: &Key, inv: &Inventory) -> Action {
        let len = self.entries(inv).len();
        match key {
            Key::Escape | Key::Ctrl('c') if !self.query.is_empty() => self.query.clear(),
            Key::Escape | Key::Ctrl('c') => return Action::Close,
            Key::Tab | Key::BackTab => {
                let step = if *key == Key::Tab { 1 } else { TABS.len() - 1 };
                self.tab = (self.tab + step) % TABS.len();
                self.selected = 0;
            }
            Key::Up if len > 0 => self.selected = (self.selected + len - 1) % len,
            Key::Down if len > 0 => self.selected = (self.selected + 1) % len,
            Key::PageUp => self.selected = self.selected.saturating_sub(PAGE),
            Key::PageDown => self.selected = (self.selected + PAGE).min(len.saturating_sub(1)),
            Key::Home => self.selected = 0,
            Key::End => self.selected = len.saturating_sub(1),
            Key::Left | Key::Right => return self.cycle(inv, *key == Key::Right),
            Key::Enter if len > 0 => {
                if self.choices(inv).is_empty() {
                    self.status = Some((Tone::Warn, self.locked_reason(inv)));
                } else {
                    let current = match self.entries(inv).get(self.selected) {
                        Some(Entry::Item(row)) => row.mode,
                        Some(Entry::Rule(rule)) => rule.mode,
                        None => Mode::On,
                    };
                    let kind = self.selected_kind(inv).unwrap_or(Kind::Tool);
                    let choice = modes(kind).iter().position(|m| *m == current).unwrap_or(0);
                    self.screen = Screen::Menu { choice };
                }
            }
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

    fn menu_key(&mut self, key: &Key, inv: &Inventory, choice: usize) -> Action {
        let choices = self.choices(inv);
        match key {
            Key::Up if !choices.is_empty() => {
                self.screen = Screen::Menu {
                    choice: (choice + choices.len() - 1) % choices.len(),
                }
            }
            Key::Down if !choices.is_empty() => {
                self.screen = Screen::Menu {
                    choice: (choice + 1) % choices.len(),
                }
            }
            Key::Enter => {
                let Some(picked) = choices.get(choice) else {
                    self.screen = Screen::List;
                    return Action::Redraw;
                };
                self.screen = Screen::List;
                if *picked == Choice::Unload {
                    if let Some(Entry::Item(row)) = self.entries(inv).get(self.selected) {
                        return Action::Unload(row.name.clone());
                    }
                }
                if let Some((change, scope)) = self.change(inv, picked) {
                    return self.write(change, scope);
                }
            }
            _ if key.is_cancel() || matches!(key, Key::Left | Key::Backspace) => {
                self.screen = Screen::List
            }
            _ => {}
        }
        Action::Redraw
    }

    /// `←`/`→` step the selected entry through the modes.
    fn cycle(&mut self, inv: &Inventory, forward: bool) -> Action {
        if self.choices(inv).is_empty() {
            if !self.entries(inv).is_empty() {
                self.status = Some((Tone::Warn, self.locked_reason(inv)));
            }
            return Action::Redraw;
        }
        let current = match self.entries(inv).get(self.selected) {
            Some(Entry::Item(row)) => row.mode,
            Some(Entry::Rule(rule)) => rule.mode,
            None => return Action::Redraw,
        };
        let list = modes(self.selected_kind(inv).unwrap_or(Kind::Tool));
        let at = list.iter().position(|m| *m == current).unwrap_or(0);
        let next = list[if forward {
            (at + 1) % list.len()
        } else {
            (at + list.len() - 1) % list.len()
        }];
        match self.change(inv, &Choice::Set(next)) {
            Some((change, scope)) => Action::Write {
                change,
                scope: scope.unwrap_or(Scope::Global),
            },
            None => Action::Redraw,
        }
    }

    fn locked_reason(&self, inv: &Inventory) -> String {
        match self.entries(inv).get(self.selected) {
            Some(Entry::Item(row)) => match &row.why {
                rows::Why::Pinned(reason) => format!("{} cannot change here: {reason}.", row.name),
                _ => format!("{} cannot change here.", row.name),
            },
            Some(Entry::Rule(rule)) => format!(
                "This rule is in {}; edit that file to change it.",
                rule.source
            ),
            None => String::new(),
        }
    }

    pub(super) fn hints(&self) -> String {
        let hints: &[(&str, &str)] = match self.screen {
            Screen::List if self.query.is_empty() => &[
                ("↑↓", "select"),
                ("←→", "change mode"),
                ("enter", "actions"),
                ("tab", "next tab"),
                ("esc", "close"),
            ],
            Screen::List => &[
                ("↑↓", "select"),
                ("←→", "change mode"),
                ("enter", "actions"),
                ("tab", "next tab"),
                ("esc", "clear search"),
            ],
            _ => &[("↑↓", "choose"), ("enter", "run"), ("esc", "back")],
        };
        let parts: Vec<String> = hints
            .iter()
            .map(|(key, what)| format!("{} {}", style::dim(key), muted(what)))
            .collect();
        format!(" {}", parts.join(&style::dim(" · ")))
    }
}
