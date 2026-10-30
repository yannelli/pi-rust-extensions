//! Raw terminal key input routed onto questionnaire transitions.

use pi_ext_sdk::Key;

use crate::config::matches_spec;
use crate::state::{EditTarget, Effect, Questionnaire, Row};

const NEWLINE_SEQUENCES: &[&str] = &["\x1b[13;2u", "\x1b\r", "\x1b\n", "\x1b[27;2;13~"];

fn is_newline(key: &Key) -> bool {
    matches!(key, Key::Unknown(raw) if NEWLINE_SEQUENCES.contains(&raw.as_str()))
}

fn is_cancel(key: &Key) -> bool {
    key.is("escape") || key.is("ctrl+c")
}

fn wrap(index: isize, total: usize) -> usize {
    if total == 0 {
        return 0;
    }
    index.rem_euclid(total as isize) as usize
}

fn typed_text(key: &Key) -> Option<String> {
    match key {
        Key::Char(c) => Some(c.to_string()),
        Key::Text(text) => Some(text.clone()),
        _ => None,
    }
}

fn digit(key: &Key) -> Option<usize> {
    match key {
        Key::Char(c @ '1'..='9') => Some(*c as usize - '1' as usize),
        _ => None,
    }
}

impl Questionnaire {
    pub fn handle_raw(&mut self, data: &str) -> Option<Effect> {
        self.handle_key(&Key::parse(data))
    }

    /// Applies one key; returns the effect the panel loop has to run.
    pub fn handle_key(&mut self, key: &Key) -> Option<Effect> {
        if matches_spec(key, &self.collapse_key) {
            self.collapsed = !self.collapsed;
            return None;
        }
        if self.collapsed {
            return is_cancel(key).then(|| self.done(true));
        }
        if self.notes_visible {
            self.notes_key_input(key);
            return self.notes_editor_effect(key);
        }
        if self.input_mode {
            return self.input_key(key);
        }
        if self.on_submit_tab() {
            return self.submit_key(key);
        }
        if self.tab_switch(key) {
            return None;
        }
        let multi = self.question()?.is_multi();
        if self.is_notes_key(key) {
            self.notes_enter();
            return None;
        }
        let total = self.current_rows().len();
        if key.is("up") {
            self.nav(wrap(self.option_index as isize - 1, total));
            return None;
        }
        if key.is("down") {
            self.nav(wrap(self.option_index as isize + 1, total));
            return None;
        }
        if multi {
            self.multi_key(key)
        } else {
            self.single_key(key)
        }
    }

    fn is_notes_key(&self, key: &Key) -> bool {
        let mut chars = self.notes_key.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => *key == Key::Char(c),
            _ => matches_spec(key, &self.notes_key),
        }
    }

    fn tab_switch(&mut self, key: &Key) -> bool {
        if !self.is_multi() {
            return false;
        }
        let total = self.total_tabs();
        let delta = if key.is("tab") || key.is("right") {
            1
        } else if key.is("shift+tab") || key.is("left") {
            -1
        } else {
            return false;
        };
        self.switch_tab(wrap(self.current_tab as isize + delta, total));
        true
    }

    fn notes_key_input(&mut self, key: &Key) {
        if is_newline(key) {
            self.notes_draft.push('\n');
        } else if key.is("backspace") {
            self.notes_draft.pop();
        } else if key.is("ctrl+u") {
            self.notes_draft.clear();
        } else if let Some(text) = typed_text(key) {
            self.notes_draft.push_str(&text);
        }
    }

    fn notes_editor_effect(&mut self, key: &Key) -> Option<Effect> {
        if is_cancel(key) || key.is("enter") {
            self.notes_exit();
        } else if key.is("ctrl+g") {
            return Some(Effect::Edit(EditTarget::Notes, self.notes_draft.clone()));
        }
        None
    }

    fn input_key(&mut self, key: &Key) -> Option<Effect> {
        if is_newline(key) {
            self.input.push('\n');
            return None;
        }
        if key.is("enter") {
            if self.question()?.is_multi() {
                return self.multi_confirm();
            }
            let answer = self.single_answer(None)?;
            return self.confirm(answer);
        }
        if key.is("ctrl+u") {
            self.clear_input();
        } else if key.is("ctrl+g") {
            return Some(Effect::Edit(EditTarget::Input, self.input.clone()));
        } else if is_cancel(key) {
            return Some(self.done(true));
        } else if key.is("up") || key.is("down") {
            let delta = if key.is("up") { -1 } else { 1 };
            let total = self.current_rows().len();
            self.nav(wrap(self.option_index as isize + delta, total));
        } else if key.is("backspace") {
            self.input.pop();
        } else if let Some(text) = typed_text(key) {
            self.input.push_str(&text);
        }
        None
    }

    fn submit_key(&mut self, key: &Key) -> Option<Effect> {
        if is_cancel(key) {
            return Some(self.done(true));
        }
        if self.tab_switch(key) {
            return None;
        }
        if key.is("up") || key.is("down") {
            self.submit_choice = 1 - self.submit_choice.min(1);
            return None;
        }
        if key.is("enter") {
            return Some(self.done(self.submit_choice == 1));
        }
        if self.is_notes_key(key) {
            self.notes_enter();
        }
        None
    }

    fn digit_row(&self, key: &Key) -> Option<(usize, Row)> {
        let index = digit(key)?;
        let row = self.current_rows().get(index).copied()?;
        row.numbered().then_some((index, row))
    }

    fn multi_key(&mut self, key: &Key) -> Option<Effect> {
        let focused = self.current_row();
        if key.is("space") {
            if let Some(Row::Option(_)) = focused {
                self.toggle(self.option_index);
            }
            return None;
        }
        if key.is("enter") {
            return match focused {
                Some(Row::Next) => self.multi_confirm(),
                Some(Row::Option(_)) => {
                    self.toggle(self.option_index);
                    None
                }
                _ => None,
            };
        }
        if is_cancel(key) {
            return Some(self.done(true));
        }
        match self.digit_row(key)? {
            (index, Row::Other) => self.nav(index),
            (index, _) => self.toggle(index),
        }
        None
    }

    fn single_key(&mut self, key: &Key) -> Option<Effect> {
        if key.is("enter") {
            let answer = self.single_answer(self.current_row())?;
            return self.confirm(answer);
        }
        if is_cancel(key) {
            return Some(self.done(true));
        }
        let (index, row) = self.digit_row(key)?;
        if row == Row::Other {
            self.nav(index);
            return None;
        }
        let answer = self.single_answer(Some(row))?;
        self.confirm(answer)
    }
}
