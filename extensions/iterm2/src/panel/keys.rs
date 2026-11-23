//! Keys of the `/iterm2` panel.

use pi_ext_sdk::Key;

use super::{Action, Edit, Row, Screen, Tab, Tone, View};
use crate::sequences::ProgramState;
use crate::themes::{Choice, Theme, hex, parse_hex};

const HEX_LEN: usize = 7;

/// The menu of a state row, in this order.
const STATE_CHOICES: usize = 3;

fn step(at: usize, count: usize, forward: bool) -> usize {
    if forward {
        (at + 1) % count
    } else {
        (at + count - 1) % count
    }
}

impl View {
    pub fn key(&mut self, key: &Key) -> Action {
        match self.screen.clone() {
            Screen::List => self.list_key(key),
            Screen::Menu { choice } => self.menu_key(key, choice),
            Screen::Hex { state, text } => self.hex_key(key, state, text),
        }
    }

    fn list_key(&mut self, key: &Key) -> Action {
        self.status = None;
        let len = self.tab.rows().len();
        match key {
            Key::Escape | Key::Ctrl('c') | Key::Char('q') => return Action::Close,
            Key::Tab | Key::BackTab => {
                let at = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
                let next = step(at, Tab::ALL.len(), *key == Key::Tab);
                self.tab = Tab::ALL[next];
                self.selected = 0;
            }
            Key::Up => self.selected = step(self.selected, len, false),
            Key::Down => self.selected = step(self.selected, len, true),
            Key::Home => self.selected = 0,
            Key::End => self.selected = len - 1,
            Key::Left | Key::Right => return self.change(*key == Key::Right),
            Key::Enter => return self.open(),
            _ => {}
        }
        Action::Redraw
    }

    /// ← and →: the previous or next theme, state color, or switch value.
    fn change(&self, forward: bool) -> Action {
        match self.row() {
            Row::Theme => {
                let at = Theme::ALL
                    .iter()
                    .position(|t| *t == self.config.palette.theme);
                let next = step(at.unwrap_or(0), Theme::ALL.len(), forward);
                Action::Save(Edit::Theme(Theme::ALL[next]))
            }
            Row::State(state) => {
                let current = self.config.palette.choice(state);
                let mut choices = vec![Choice::Theme, Choice::Profile];
                if let Choice::Custom(_) = current {
                    choices.push(current);
                }
                let at = choices.iter().position(|c| *c == current).unwrap_or(0);
                let next = choices[step(at, choices.len(), forward)];
                Action::Save(Edit::Color(state, next))
            }
            Row::Feature(feature) => Action::Save(Edit::Feature(feature, !feature.on(&self.value))),
            Row::Reset => Action::Redraw,
        }
    }

    /// Enter on a row: its menu, or the reset.
    fn open(&mut self) -> Action {
        let choice = match self.row() {
            Row::Reset => return Action::Save(Edit::ResetColors),
            Row::Theme => {
                let theme = self.config.palette.theme;
                Theme::ALL.iter().position(|t| *t == theme).unwrap_or(0)
            }
            Row::State(state) => match self.config.palette.choice(state) {
                Choice::Theme => 0,
                Choice::Profile => 1,
                Choice::Custom(_) => 2,
            },
            Row::Feature(feature) => usize::from(!feature.on(&self.value)),
        };
        self.screen = Screen::Menu { choice };
        Action::Redraw
    }

    /// The number of items in the menu of the selected row.
    pub(super) fn menu_len(&self) -> usize {
        match self.row() {
            Row::Theme => Theme::ALL.len(),
            Row::State(_) => STATE_CHOICES,
            Row::Feature(_) => 2,
            Row::Reset => 0,
        }
    }

    fn menu_key(&mut self, key: &Key, choice: usize) -> Action {
        let count = self.menu_len().max(1);
        match key {
            Key::Up => {
                self.screen = Screen::Menu {
                    choice: step(choice, count, false),
                }
            }
            Key::Down => {
                self.screen = Screen::Menu {
                    choice: step(choice, count, true),
                }
            }
            Key::Enter => {
                self.screen = Screen::List;
                return self.pick(choice);
            }
            _ if key.is_cancel() || matches!(key, Key::Left | Key::Backspace) => {
                self.screen = Screen::List;
            }
            _ => {}
        }
        Action::Redraw
    }

    fn pick(&mut self, choice: usize) -> Action {
        match self.row() {
            Row::Theme => Action::Save(Edit::Theme(Theme::ALL[choice.min(Theme::ALL.len() - 1)])),
            Row::State(state) => match choice {
                0 => Action::Save(Edit::Color(state, Choice::Theme)),
                1 => Action::Save(Edit::Color(state, Choice::Profile)),
                _ => {
                    let text = self.config.palette.color(state).map(hex);
                    self.screen = Screen::Hex {
                        state,
                        text: text.unwrap_or_default(),
                    };
                    Action::Redraw
                }
            },
            Row::Feature(feature) => Action::Save(Edit::Feature(feature, choice == 0)),
            Row::Reset => Action::Redraw,
        }
    }

    fn hex_key(&mut self, key: &Key, state: ProgramState, mut text: String) -> Action {
        self.status = None;
        match key {
            Key::Escape | Key::Ctrl('c') => {
                self.screen = Screen::List;
                return Action::Redraw;
            }
            Key::Enter => match parse_hex(&text) {
                Some(rgb) => {
                    self.screen = Screen::List;
                    return Action::Save(Edit::Color(state, Choice::Custom(rgb)));
                }
                None => {
                    let hint = "Type a color as #rrggbb, for example #1b506a.";
                    self.status = Some((Tone::Warn, hint.into()));
                }
            },
            Key::Backspace => {
                text.pop();
            }
            Key::Char(c) => push_hex(&mut text, &c.to_string()),
            Key::Text(pasted) => push_hex(&mut text, pasted),
            _ => {}
        }
        self.screen = Screen::Hex { state, text };
        Action::Redraw
    }

    /// The key hints of the current screen.
    pub(super) fn hints(&self) -> Vec<&'static str> {
        match &self.screen {
            Screen::List => {
                let enter = if self.row() == Row::Reset {
                    "enter reset"
                } else {
                    "enter choose"
                };
                vec!["↑↓ move", "tab switch", "←→ change", enter, "esc close"]
            }
            Screen::Menu { .. } => vec!["↑↓ choose", "enter apply", "esc back"],
            Screen::Hex { .. } => vec!["type #rrggbb", "enter save", "esc cancel"],
        }
    }
}

/// Adds the hex digits of `input` to `text`, with one leading `#`, up to `#rrggbb`.
fn push_hex(text: &mut String, input: &str) {
    for c in input.chars() {
        if text.is_empty() && c != '#' {
            text.push('#');
        }
        if c.is_ascii_hexdigit() && text.len() < HEX_LEN {
            text.push(c.to_ascii_lowercase());
        } else if c == '#' && text.is_empty() {
            text.push(c);
        }
    }
}
