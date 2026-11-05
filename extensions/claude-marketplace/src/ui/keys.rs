//! Keys of the `/claude-plugins` panel. Typing filters; nothing here waits on I/O.

use pi_ext_sdk::Key;

use super::{Action, Entry, Inventory, Op, Screen, Special, TABS, Tone, View};
use crate::env::Scope;

const PAGE: usize = 10;

impl View {
    pub fn key(&mut self, key: &Key, inv: &Inventory) -> Action {
        match self.screen.clone() {
            Screen::List => self.list_key(key, inv),
            Screen::Menu { choice } => self.menu_key(key, inv, choice),
            Screen::Confirm { op, .. } => match key {
                Key::Enter => {
                    self.screen = Screen::List;
                    Action::Run(op)
                }
                _ => {
                    self.screen = Screen::List;
                    self.status = Some((Tone::Info, "Cancelled.".into()));
                    Action::Redraw
                }
            },
            Screen::Add { source, scope } => self.add_key(key, source, scope),
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
            Key::Left | Key::Right => return self.toggle(inv),
            Key::Enter if len > 0 => return self.open(inv),
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

    /// Enter on a list row: the actions pane, the add line, or a bulk operation.
    fn open(&mut self, inv: &Inventory) -> Action {
        match self.entries(inv).get(self.selected) {
            Some(Entry::Special(Special::Add)) => {
                self.screen = Screen::Add {
                    source: String::new(),
                    scope: Scope::User,
                };
            }
            Some(Entry::Special(Special::UpdateAll)) => return Action::Run(Op::UpdateAll),
            Some(Entry::Special(Special::Import)) => return Action::Run(Op::Import),
            Some(Entry::Plugin(p)) if self.choices(inv).is_empty() => {
                let why = if p.row.reasons.is_empty() {
                    "pi cannot install this plugin.".to_owned()
                } else {
                    format!(
                        "pi cannot install this plugin: {}.",
                        p.row.reasons.join("; ")
                    )
                };
                self.status = Some((Tone::Warn, why));
            }
            Some(_) => self.screen = Screen::Menu { choice: 0 },
            None => {}
        }
        Action::Redraw
    }

    /// ← and →: turn an installed plugin on or off, or a marketplace's auto-update.
    fn toggle(&mut self, inv: &Inventory) -> Action {
        match self.entries(inv).get(self.selected) {
            Some(Entry::Plugin(p)) if p.installed() => Action::Run(Op::SetEnabled {
                id: p.id(),
                scope: p.row.scope,
                enabled: !p.enabled(),
            }),
            Some(Entry::Market(m)) if m.git => Action::Run(Op::AutoUpdate {
                name: m.name.clone(),
                on: !m.auto_update,
            }),
            _ => Action::Redraw,
        }
    }

    fn menu_key(&mut self, key: &Key, inv: &Inventory, choice: usize) -> Action {
        let choices = self.choices(inv);
        let len = choices.len().max(1);
        match key {
            Key::Up => {
                self.screen = Screen::Menu {
                    choice: (choice + len - 1) % len,
                }
            }
            Key::Down => {
                self.screen = Screen::Menu {
                    choice: (choice + 1) % len,
                }
            }
            Key::Enter => {
                let picked = choices.into_iter().nth(choice);
                self.screen = Screen::List;
                if let Some(picked) = picked {
                    if let Some(op) = picked.op {
                        if picked.confirm {
                            let question =
                                format!("{}? Enter confirms; any other key cancels.", picked.label);
                            self.screen = Screen::Confirm { op, question };
                        } else {
                            return Action::Run(op);
                        }
                    }
                }
            }
            _ if key.is_cancel() || matches!(key, Key::Left | Key::Backspace) => {
                self.screen = Screen::List;
            }
            _ => {}
        }
        Action::Redraw
    }

    fn add_key(&mut self, key: &Key, mut source: String, mut scope: Scope) -> Action {
        match key {
            Key::Enter if !source.trim().is_empty() => {
                self.screen = Screen::List;
                return Action::Run(Op::AddMarket {
                    source: source.trim().to_owned(),
                    scope,
                });
            }
            Key::Enter => {}
            Key::Tab | Key::BackTab => {
                let scopes = self.scopes();
                let at = scopes.iter().position(|s| *s == scope).unwrap_or(0);
                scope = scopes[(at + 1) % scopes.len()];
            }
            Key::Backspace => {
                source.pop();
            }
            Key::Char(' ') => {}
            Key::Char(c) => source.push(*c),
            Key::Text(text) => source.push_str(text.trim()),
            _ if key.is_cancel() => {
                self.screen = Screen::List;
                return Action::Redraw;
            }
            _ => {}
        }
        self.screen = Screen::Add { source, scope };
        Action::Redraw
    }

    /// The key hints of the current screen.
    pub fn hints(&self) -> String {
        match &self.screen {
            Screen::List => {
                let toggle = match self.tab() {
                    super::Tab::Installed => " · ←→ on/off",
                    super::Tab::Marketplaces => " · ←→ auto-update",
                    super::Tab::Discover => "",
                };
                let esc = if self.query.is_empty() {
                    "esc close"
                } else {
                    "esc clear search"
                };
                format!(" ↑↓ move · tab switch · enter actions{toggle} · type to search · {esc}")
            }
            Screen::Menu { .. } => " ↑↓ choose · enter run · esc back".into(),
            Screen::Confirm { .. } => " enter confirm · any other key cancels".into(),
            Screen::Add { .. } if self.trusted => {
                " type a source · tab scope · enter add · esc cancel".into()
            }
            Screen::Add { .. } => " type a source · enter add · esc cancel".into(),
        }
    }
}
