//! A list with fuzzy search inside the panel: one choice (models) or toggles (tools).

use pi_ext_sdk::{Key, style};

use crate::ui::line::LineEdit;
use crate::ui::paint::{Paint, window};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub value: String,
    pub note: String,
}

impl Item {
    pub fn new(value: impl Into<String>, note: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            note: note.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    Pending,
    Cancel,
    One(String),
    /// `None` is the "all" row: the agent inherits every tool.
    Many(Option<Vec<String>>),
}

#[derive(Debug, Clone)]
pub struct Picker {
    pub title: String,
    pub items: Vec<Item>,
    /// Toggles instead of one choice.
    pub multi: bool,
    /// A first row that means "every item" (multi only); `true` when it is on.
    pub all: Option<bool>,
    pub checked: Vec<String>,
    pub search: LineEdit,
    pub row: usize,
}

/// A row of the picker as drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    All,
    Item(usize),
    /// Use the search text as a value that is not in the list.
    Custom(String),
}

/// Subsequence match: higher is better; `None` when `needle` is not in `hay` in order.
pub fn fuzzy(needle: &str, hay: &str) -> Option<i64> {
    let needle: Vec<char> = needle
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let hay: Vec<char> = hay.to_lowercase().chars().collect();
    if needle.is_empty() {
        return Some(0);
    }
    let (mut score, mut at, mut last) = (0i64, 0usize, None::<usize>);
    for c in needle {
        let found = (at..hay.len()).find(|&i| hay[i] == c)?;
        let boundary = found == 0 || matches!(hay[found - 1], '/' | '-' | '_' | '.' | ' ' | ':');
        score += 1;
        if boundary {
            score += 8;
        }
        if last.is_some_and(|l| l + 1 == found) {
            score += 5;
        }
        score -= (found - at).min(10) as i64;
        last = Some(found);
        at = found + 1;
    }
    Some(score * 100 - hay.len() as i64)
}

impl Picker {
    pub fn one(title: &str, items: Vec<Item>, current: Option<&str>) -> Self {
        let mut picker = Self::base(title, items, false);
        picker.row = current
            .and_then(|c| picker.items.iter().position(|i| i.value == c))
            .unwrap_or(0);
        picker
    }

    /// Toggles; `current` `None` turns the "all" row on when `all` is `Some`.
    pub fn many(title: &str, mut items: Vec<Item>, current: Option<&[String]>, all: bool) -> Self {
        for value in current.into_iter().flatten() {
            if !items.iter().any(|i| &i.value == value) {
                items.push(Item::new(value.clone(), "not loaded in this session"));
            }
        }
        let mut picker = Self::base(title, items, true);
        picker.all = all.then_some(current.is_none());
        picker.checked = current.map(<[String]>::to_vec).unwrap_or_default();
        picker
    }

    fn base(title: &str, items: Vec<Item>, multi: bool) -> Self {
        Self {
            title: title.to_owned(),
            items,
            multi,
            all: None,
            checked: Vec::new(),
            search: LineEdit::default(),
            row: 0,
        }
    }

    fn rows(&self) -> Vec<Row> {
        let needle = self.search.text.trim();
        let mut rows = Vec::new();
        if self.all.is_some() && needle.is_empty() {
            rows.push(Row::All);
        }
        let mut scored: Vec<(i64, usize)> = (0..self.items.len())
            .filter_map(|i| {
                let item = &self.items[i];
                let hay = format!("{} {}", item.value, item.note);
                let score =
                    fuzzy(needle, &item.value).or_else(|| fuzzy(needle, &hay).map(|s| s - 5000));
                score.map(|s| (s, i))
            })
            .collect();
        if !needle.is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        rows.extend(scored.into_iter().map(|(_, i)| Row::Item(i)));
        if !needle.is_empty() && !self.items.iter().any(|i| i.value == needle) {
            rows.push(Row::Custom(needle.to_owned()));
        }
        rows
    }

    /// The values the search shows, best match first.
    pub fn shown(&self) -> Vec<String> {
        let rows = self.rows().into_iter();
        rows.filter_map(|r| match r {
            Row::Item(i) => Some(self.items[i].value.clone()),
            _ => None,
        })
        .collect()
    }

    fn toggle(&mut self, value: &str) {
        if let Some(at) = self.checked.iter().position(|v| v == value) {
            self.checked.remove(at);
        } else {
            self.checked.push(value.to_owned());
        }
        if let Some(all) = self.all.as_mut() {
            *all = false;
        }
    }

    fn done(&self) -> Picked {
        if self.all == Some(true) {
            return Picked::Many(None);
        }
        let order = |v: &String| self.items.iter().position(|i| &i.value == v);
        let mut values = self.checked.clone();
        values.sort_by_key(order);
        Picked::Many(Some(values))
    }

    pub fn key(&mut self, key: &Key) -> Picked {
        let rows = self.rows();
        let last = rows.len().saturating_sub(1);
        self.row = self.row.min(last);
        let current = rows.get(self.row).cloned();
        match key {
            Key::Escape if !self.search.text.is_empty() => self.search = LineEdit::default(),
            Key::Escape | Key::Ctrl('c') => return Picked::Cancel,
            Key::Up => self.row = self.row.saturating_sub(1),
            Key::Down => self.row = (self.row + 1).min(last),
            Key::PageUp => self.row = self.row.saturating_sub(10),
            Key::PageDown => self.row = (self.row + 10).min(last),
            Key::Enter if !self.multi => {
                return match current {
                    Some(Row::Item(i)) => Picked::One(self.items[i].value.clone()),
                    Some(Row::Custom(text)) => Picked::One(text),
                    _ => Picked::Pending,
                };
            }
            Key::Char(' ') | Key::Tab if self.multi => match current {
                Some(Row::All) => {
                    let on = self.all.is_some_and(|a| !a);
                    self.all = Some(on);
                }
                Some(Row::Item(i)) => self.toggle(&self.items[i].value.clone()),
                Some(Row::Custom(text)) => {
                    self.items.push(Item::new(text.clone(), "added by you"));
                    self.toggle(&text);
                    self.search = LineEdit::default();
                }
                None => {}
            },
            Key::Ctrl('a') if self.multi => {
                let shown = self.shown();
                let all = shown.iter().all(|v| self.checked.contains(v));
                for value in shown {
                    if all || !self.checked.contains(&value) {
                        self.toggle(&value);
                    }
                }
            }
            Key::Enter => return self.done(),
            other => {
                if self.search.key(other) {
                    self.row = 0;
                }
            }
        }
        Picked::Pending
    }

    pub fn lines(&self, paint: &Paint, width: usize, rows: usize) -> Vec<String> {
        let count = if self.multi {
            match self.all {
                Some(true) => "all".to_owned(),
                _ => format!("{} on", self.checked.len()),
            }
        } else {
            format!("{} of {}", self.shown().len(), self.items.len())
        };
        let mut lines = vec![paint.header(&self.title, &paint.muted(&count), width)];
        lines.push(if self.search.text.is_empty() {
            paint.dim("  ⌕ type to search")
        } else {
            format!(
                "  {} {}",
                paint.accent("⌕"),
                self.search.render(width.saturating_sub(6))
            )
        });
        let all_rows = self.rows();
        if all_rows.is_empty() {
            lines.push(paint.muted("  Nothing matches."));
        }
        let value_width = self.items.iter().map(|i| i.value.chars().count()).max();
        let value_width = value_width.unwrap_or(8).clamp(8, (width / 2).max(8));
        let rows = rows.saturating_sub(2).max(3);
        for i in window(self.row, all_rows.len(), rows) {
            let text = self.row_text(paint, &all_rows[i], value_width, width);
            lines.push(paint.row(&text, i == self.row, width));
        }
        lines
    }

    fn row_text(&self, paint: &Paint, row: &Row, value_width: usize, width: usize) -> String {
        let check = |on: bool| match (self.multi, on) {
            (false, _) => String::new(),
            (true, true) => format!("{} ", paint.accent("[✓]")),
            (true, false) => format!("{} ", paint.dim("[ ]")),
        };
        match row {
            Row::All => format!(
                "{}{}",
                check(self.all == Some(true)),
                paint.accent("all tools · inherit every tool pi has")
            ),
            Row::Item(i) => {
                let item = &self.items[*i];
                let on = self.checked.contains(&item.value) && self.all != Some(true);
                let value = style::fit(&item.value, value_width);
                let note_width = width.saturating_sub(value_width + 10);
                let note = style::truncate(item.note.lines().next().unwrap_or(""), note_width);
                format!("{}{value}  {}", check(on), paint.muted(&note))
            }
            Row::Custom(text) => paint.accent(&format!("+ use \"{text}\"")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> Vec<Item> {
        [
            "anthropic/claude-sonnet-4-6",
            "anthropic/claude-haiku-4-5",
            "openai/gpt-5",
            "openai/gpt-5-mini",
        ]
        .into_iter()
        .map(|m| Item::new(m, ""))
        .collect()
    }

    #[test]
    fn fuzzy_ranks_boundaries_and_runs() {
        assert!(fuzzy("son", "anthropic/claude-sonnet-4-6").is_some());
        assert!(fuzzy("xyz", "openai/gpt-5").is_none());
        let a = fuzzy("gpt5", "openai/gpt-5").unwrap();
        let b = fuzzy("gpt5", "openai/gpt-5-mini").unwrap();
        assert!(a > b, "the shorter match wins");
    }

    #[test]
    fn search_picks_the_best_match_and_custom_values() {
        let mut picker = Picker::one("Model", models(), None);
        for c in "haiku".chars() {
            picker.key(&Key::Char(c));
        }
        assert_eq!(picker.shown()[0], "anthropic/claude-haiku-4-5");
        assert_eq!(
            picker.key(&Key::Enter),
            Picked::One("anthropic/claude-haiku-4-5".into())
        );
        let mut picker = Picker::one("Model", models(), None);
        picker.key(&Key::Text("local/llama".into()));
        assert_eq!(picker.key(&Key::Enter), Picked::One("local/llama".into()));
    }

    #[test]
    fn toggles_keep_item_order_and_all_row() {
        let items = ["read", "grep", "bash"].map(|t| Item::new(t, "")).to_vec();
        let current = vec!["bash".to_owned()];
        let mut picker = Picker::many("Tools", items, Some(&current), true);
        assert_eq!(picker.all, Some(false));
        picker.key(&Key::Down);
        picker.key(&Key::Char(' '));
        assert_eq!(
            picker.key(&Key::Enter),
            Picked::Many(Some(vec!["read".into(), "bash".into()]))
        );
        picker.row = 0;
        picker.key(&Key::Char(' '));
        assert_eq!(picker.key(&Key::Enter), Picked::Many(None));
    }
}
