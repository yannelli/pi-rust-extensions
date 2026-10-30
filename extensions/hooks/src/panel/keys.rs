use pi_ext_sdk::Key;

use super::{Action, DetailAction, Mode, PAGE, Row, Tab, TrustChange, View};
use crate::manage::PROJECT_UNTRUSTED;
use crate::model::Scope;

impl View {
    pub fn key(&mut self, key: &Key, rows: &[Row]) -> Action {
        self.status = None;
        match self.mode.clone() {
            Mode::List => self.list_key(key, rows),
            Mode::Search => self.search_key(key, rows),
            Mode::Details { hash, action } => self.details_key(key, rows, hash, action),
            Mode::ConfirmRemove { hash } => self.confirm_key(key, hash),
            Mode::AddScope => self.add_key(key),
        }
    }

    fn list_key(&mut self, key: &Key, rows: &[Row]) -> Action {
        if key.is("escape") && !self.query.is_empty() {
            self.query.clear();
            self.selected = 0;
            return Action::Redraw;
        }
        if key.is_cancel() {
            return Action::Close;
        }
        if self.move_key(key, rows) {
            return Action::Redraw;
        }
        let hash = self.selected_row(rows).map(|r| r.def.hash.clone());
        match key {
            Key::Tab | Key::BackTab => self.cycle_tab(*key == Key::Tab),
            Key::Char('/') => self.mode = Mode::Search,
            Key::Char('a') => self.mode = Mode::AddScope,
            Key::Char('t') => return Action::TrustAll,
            Key::Char('s') => return Action::Sources,
            Key::Char('c') => return Action::LoadScope,
            Key::Char(' ') => return hash.map_or(Action::Redraw, Action::Toggle),
            Key::Enter | Key::Right => {
                if let Some(hash) = hash {
                    self.mode = Mode::Details { hash, action: 0 };
                }
            }
            Key::Char('d') | Key::Delete => {
                if let Some(hash) = hash {
                    self.mode = Mode::ConfirmRemove { hash };
                }
            }
            _ => {}
        }
        Action::Redraw
    }

    /// Up/Down wrap; Home, End, PageUp, and PageDown stop at the ends.
    fn move_key(&mut self, key: &Key, rows: &[Row]) -> bool {
        let len = self.visible(rows).len();
        let last = len.saturating_sub(1);
        self.selected = match key {
            Key::Up if len > 0 => self.selected.checked_sub(1).unwrap_or(last),
            Key::Down if len > 0 => (self.selected + 1) % len,
            Key::Home => 0,
            Key::End => last,
            Key::PageUp => self.selected.saturating_sub(PAGE),
            Key::PageDown => (self.selected + PAGE).min(last),
            Key::Up | Key::Down => 0,
            _ => return false,
        };
        true
    }

    fn cycle_tab(&mut self, forward: bool) {
        let index = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        let step = if forward { 1 } else { Tab::ALL.len() - 1 };
        self.tab = Tab::ALL[(index + step) % Tab::ALL.len()];
        self.selected = 0;
    }

    fn search_key(&mut self, key: &Key, rows: &[Row]) -> Action {
        match key {
            Key::Escape => {
                self.query.clear();
                self.mode = Mode::List;
            }
            Key::Enter | Key::Tab => self.mode = Mode::List,
            Key::Ctrl('c') => return Action::Close,
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
            _ => {
                self.move_key(key, rows);
            }
        }
        Action::Redraw
    }

    fn details_key(&mut self, key: &Key, rows: &[Row], hash: String, action: usize) -> Action {
        let Some(row) = rows.iter().find(|r| r.def.hash == hash) else {
            self.mode = Mode::List;
            return Action::Redraw;
        };
        let count = DetailAction::ALL.len();
        match key {
            Key::Escape | Key::Left | Key::Backspace | Key::Char('q') => self.mode = Mode::List,
            Key::Ctrl('c') => return Action::Close,
            Key::Up => {
                let action = action.checked_sub(1).unwrap_or(count - 1);
                self.mode = Mode::Details { hash, action };
            }
            Key::Down => {
                let action = (action + 1) % count;
                self.mode = Mode::Details { hash, action };
            }
            Key::Char(' ') => return Action::Toggle(hash),
            Key::Char('d') | Key::Delete => self.mode = Mode::ConfirmRemove { hash },
            Key::Enter => return self.run_detail(DetailAction::ALL[action], row, hash),
            _ => {}
        }
        Action::Redraw
    }

    fn run_detail(&mut self, action: DetailAction, row: &Row, hash: String) -> Action {
        let change = match action {
            DetailAction::Toggle => return Action::Toggle(row.def.hash.clone()),
            DetailAction::Remove => {
                self.mode = Mode::ConfirmRemove { hash };
                return Action::Redraw;
            }
            DetailAction::TrustAlways => TrustChange::Always,
            DetailAction::AllowOnce => TrustChange::Once,
            DetailAction::Untrust => TrustChange::Untrust,
            DetailAction::Deny => TrustChange::Deny,
        };
        Action::Trust(hash, change)
    }

    fn confirm_key(&mut self, key: &Key, hash: String) -> Action {
        match key {
            Key::Char('y') | Key::Char('Y') => {
                self.mode = Mode::List;
                Action::Remove(hash)
            }
            Key::Char('n') | Key::Char('N') | Key::Escape => {
                self.mode = Mode::List;
                Action::Redraw
            }
            Key::Ctrl('c') => Action::Close,
            _ => Action::Redraw,
        }
    }

    fn add_key(&mut self, key: &Key) -> Action {
        let scope = match key {
            Key::Char('p') => Scope::Project,
            Key::Char('g') => Scope::Global,
            Key::Escape | Key::Char('n') => {
                self.mode = Mode::List;
                return Action::Redraw;
            }
            Key::Ctrl('c') => return Action::Close,
            _ => return Action::Redraw,
        };
        self.mode = Mode::List;
        if scope == Scope::Project && !self.project_trusted {
            self.status = Some(format!("Cannot add: {PROJECT_UNTRUSTED}"));
            return Action::Redraw;
        }
        Action::Add(scope)
    }
}
