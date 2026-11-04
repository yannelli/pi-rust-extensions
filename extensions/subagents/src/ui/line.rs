//! A one-line text input inside the panel: cursor keys, word delete, and paste.

use pi_ext_sdk::{Key, style};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineEdit {
    pub text: String,
    /// The cursor as a character index.
    cursor: usize,
}

impl LineEdit {
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            cursor: text.chars().count(),
        }
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    fn byte(&self, at: usize) -> usize {
        self.text
            .char_indices()
            .nth(at)
            .map_or(self.text.len(), |(i, _)| i)
    }

    fn insert(&mut self, text: &str) {
        let at = self.byte(self.cursor);
        let clean: String = text.chars().filter(|c| !c.is_control()).collect();
        self.text.insert_str(at, &clean);
        self.cursor += clean.chars().count();
    }

    fn remove(&mut self, from: usize, to: usize) {
        let (start, end) = (self.byte(from), self.byte(to));
        self.text.replace_range(start..end, "");
        self.cursor = from;
    }

    fn word_start(&self) -> usize {
        let chars: Vec<char> = self.text.chars().collect();
        let mut at = self.cursor;
        while at > 0 && chars[at - 1].is_whitespace() {
            at -= 1;
        }
        while at > 0 && !chars[at - 1].is_whitespace() {
            at -= 1;
        }
        at
    }

    /// `true` when the key edits the text or moves the cursor.
    pub fn key(&mut self, key: &Key) -> bool {
        match key {
            Key::Char(c) => self.insert(&c.to_string()),
            Key::Text(text) => self.insert(text),
            Key::Backspace if self.cursor > 0 => self.remove(self.cursor - 1, self.cursor),
            Key::Delete if self.cursor < self.len() => self.remove(self.cursor, self.cursor + 1),
            Key::Left => self.cursor = self.cursor.saturating_sub(1),
            Key::Right => self.cursor = (self.cursor + 1).min(self.len()),
            Key::Home | Key::Ctrl('a') => self.cursor = 0,
            Key::End | Key::Ctrl('e') => self.cursor = self.len(),
            Key::Ctrl('u') => self.remove(0, self.cursor),
            Key::Ctrl('k') => self.remove(self.cursor, self.len()),
            Key::Ctrl('w') | Key::Alt('\x7f') => self.remove(self.word_start(), self.cursor),
            Key::Backspace | Key::Delete => {}
            _ => return false,
        }
        true
    }

    /// The text with an inverse-video cursor, cut to `width` columns around the cursor.
    pub fn render(&self, width: usize) -> String {
        let chars: Vec<char> = self.text.chars().collect();
        let width = width.max(4);
        let start = self.cursor.saturating_sub(width.saturating_sub(2));
        let before: String = chars[start..self.cursor].iter().collect();
        let under = chars.get(self.cursor).map_or(" ".into(), char::to_string);
        let after: String = chars.iter().skip(self.cursor + 1).collect();
        let line = format!("{before}{}{after}", style::inverse(&under));
        style::truncate(&line, width)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(edit: &mut LineEdit, keys: &[Key]) {
        for key in keys {
            edit.key(key);
        }
    }

    #[test]
    fn edits_at_the_cursor() {
        let mut edit = LineEdit::new("helo");
        typed(&mut edit, &[Key::Left, Key::Char('l'), Key::End]);
        assert_eq!(edit.text, "hello");
        typed(&mut edit, &[Key::Text(" big world".into()), Key::Ctrl('w')]);
        assert_eq!(edit.text, "hello big ");
        typed(&mut edit, &[Key::Home, Key::Delete, Key::Ctrl('k')]);
        assert_eq!(edit.text, "");
        assert!(!edit.key(&Key::Enter));
        let edit = LineEdit::new("ab");
        assert_eq!(style::strip(&edit.render(20)), "ab ");
    }
}
