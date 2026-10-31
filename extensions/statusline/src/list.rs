//! A keyboard settings list: each item opens a searchable list of values.

use pi_ext_sdk::Key;
use pi_ext_sdk::style::{self, truncate};

use crate::colors::ThemeColor;

const VISIBLE: usize = 10;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingItem {
    pub id: String,
    pub label: String,
    pub description: String,
    pub current: String,
    /// The values to pick from; empty for an action that Enter runs.
    pub values: Vec<String>,
    /// Enter reports [`ListOutput::Open`] instead of opening the values.
    pub custom: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ListOutput {
    None,
    Change(String, String),
    Open(String),
    Cancel,
}

#[derive(Debug, Default)]
struct Submenu {
    item: usize,
    filter: String,
    cursor: usize,
}

#[derive(Debug, Default)]
pub struct SettingsList {
    items: Vec<SettingItem>,
    selected: usize,
    submenu: Option<Submenu>,
}

pub fn value_label(value: &str) -> &str {
    match value {
        "" => "none",
        " " => "1 space",
        "  " => "2 spaces",
        value => value,
    }
}

impl SettingsList {
    pub fn new(items: Vec<SettingItem>) -> Self {
        Self {
            items,
            selected: 0,
            submenu: None,
        }
    }

    /// Replaces the items and selects `id`, or keeps the position when `id` is gone.
    pub fn replace(&mut self, items: Vec<SettingItem>, id: &str) {
        self.items = items;
        self.submenu = None;
        match self.items.iter().position(|item| item.id == id) {
            Some(index) => self.selected = index,
            None => self.selected = self.selected.min(self.items.len().saturating_sub(1)),
        }
    }

    pub fn select_item(&mut self, id: &str) {
        if let Some(index) = self.items.iter().position(|item| item.id == id) {
            self.selected = index;
        }
    }

    /// The item id and the value under the cursor of the open value list.
    pub fn highlighted(&self) -> Option<(String, String)> {
        let submenu = self.submenu.as_ref()?;
        let value = self.filtered(submenu).into_iter().nth(submenu.cursor)?;
        Some((self.items[submenu.item].id.clone(), value))
    }

    fn filtered(&self, submenu: &Submenu) -> Vec<String> {
        let filter = submenu.filter.to_lowercase();
        self.items[submenu.item]
            .values
            .iter()
            .filter(|value| {
                filter.is_empty() || value_label(value).to_lowercase().contains(&filter)
            })
            .cloned()
            .collect()
    }

    pub fn handle_key(&mut self, key: &Key) -> ListOutput {
        if let Some(mut submenu) = self.submenu.take() {
            let values = self.filtered(&submenu);
            match key {
                Key::Escape | Key::Ctrl('c') => return ListOutput::None,
                Key::Enter => {
                    return match values.get(submenu.cursor) {
                        Some(value) => {
                            ListOutput::Change(self.items[submenu.item].id.clone(), value.clone())
                        }
                        None => ListOutput::None,
                    };
                }
                Key::Up => {
                    submenu.cursor = submenu
                        .cursor
                        .checked_sub(1)
                        .unwrap_or(values.len().saturating_sub(1))
                }
                Key::Down => {
                    submenu.cursor = if submenu.cursor + 1 >= values.len() {
                        0
                    } else {
                        submenu.cursor + 1
                    }
                }
                Key::Backspace => {
                    submenu.filter.pop();
                    submenu.cursor = 0;
                }
                Key::Char(c) => {
                    submenu.filter.push(*c);
                    submenu.cursor = 0;
                }
                Key::Text(text) => {
                    submenu.filter.push_str(text);
                    submenu.cursor = 0;
                }
                _ => {}
            }
            self.submenu = Some(submenu);
            return ListOutput::None;
        }
        let count = self.items.len();
        match key {
            _ if key.is_cancel() => ListOutput::Cancel,
            Key::Up if count > 0 => {
                self.selected = self.selected.checked_sub(1).unwrap_or(count - 1);
                ListOutput::None
            }
            Key::Down if count > 0 => {
                self.selected = (self.selected + 1) % count;
                ListOutput::None
            }
            Key::Enter if count > 0 => {
                let item = &self.items[self.selected];
                if item.custom {
                    return ListOutput::Open(item.id.clone());
                }
                if item.values.is_empty() {
                    return ListOutput::Change(item.id.clone(), "yes".into());
                }
                let cursor = item
                    .values
                    .iter()
                    .position(|value| *value == item.current)
                    .unwrap_or(0);
                self.submenu = Some(Submenu {
                    item: self.selected,
                    filter: String::new(),
                    cursor,
                });
                ListOutput::None
            }
            _ => ListOutput::None,
        }
    }

    pub fn render(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(submenu) = &self.submenu {
            let item = &self.items[submenu.item];
            lines.push(style::bold(&item.label));
            lines.push(style::dim(&item.description));
            lines.push(format!("Search: {}", submenu.filter));
            let values = self.filtered(submenu);
            let start = submenu.cursor.saturating_sub(VISIBLE - 1);
            for (index, value) in values.iter().enumerate().skip(start).take(VISIBLE) {
                let mark = if *value == item.current { " ✓" } else { "" };
                let label = format!("{}{mark}", value_label(value));
                lines.push(if index == submenu.cursor {
                    ThemeColor::Accent.paint(&format!("→ {label}"))
                } else {
                    format!("  {label}")
                });
            }
            if values.is_empty() {
                lines.push(style::dim("  No match"));
            }
            lines.push(style::dim("Type to search · Enter select · Esc back"));
        } else {
            let label_width = self
                .items
                .iter()
                .map(|item| style::visible_width(&item.label))
                .max()
                .unwrap_or(0);
            let start = self.selected.saturating_sub(VISIBLE - 1);
            for (index, item) in self.items.iter().enumerate().skip(start).take(VISIBLE) {
                let pad = " ".repeat(label_width - style::visible_width(&item.label));
                let text = format!("{}{pad}  {}", item.label, item.current);
                lines.push(if index == self.selected {
                    ThemeColor::Accent.paint(&format!("→ {text}"))
                } else {
                    format!("  {text}")
                });
            }
            if let Some(item) = self.items.get(self.selected) {
                lines.push(style::dim(&item.description));
            }
            lines.push(style::dim("↑↓ move · Enter change · Esc close"));
        }
        lines
            .into_iter()
            .map(|line| truncate(&line, width))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, values: &[&str]) -> SettingItem {
        SettingItem {
            id: id.into(),
            label: id.into(),
            values: values.iter().map(|v| v.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn submenu_filters_and_picks() {
        let mut list =
            SettingsList::new(vec![item("a", &["one", "two", "three"]), item("go", &[])]);
        assert_eq!(list.handle_key(&Key::Enter), ListOutput::None);
        assert_eq!(list.handle_key(&Key::Text("th".into())), ListOutput::None);
        assert!(list.render(40).iter().any(|line| line.contains("three")));
        assert_eq!(
            list.handle_key(&Key::Enter),
            ListOutput::Change("a".into(), "three".into())
        );
        assert_eq!(list.handle_key(&Key::Down), ListOutput::None);
        assert_eq!(
            list.handle_key(&Key::Enter),
            ListOutput::Change("go".into(), "yes".into())
        );
        assert_eq!(list.handle_key(&Key::Escape), ListOutput::Cancel);
        list.replace(vec![item("x", &[])], "missing");
        assert_eq!(
            list.handle_key(&Key::Enter),
            ListOutput::Change("x".into(), "yes".into())
        );
    }
}
