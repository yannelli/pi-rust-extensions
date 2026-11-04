//! Lists in the panel with type-to-filter: a checklist (space checks, enter finishes) and a
//! single choice (enter chooses).

use std::collections::BTreeSet;

use pi_ext_sdk::prelude::*;

use crate::ui::home::row_line;
use crate::ui::theme::hints;
use crate::ui::{Screen, Ui, one_line, window};

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
    Inherit,
    List(Vec<String>),
}

struct List<'a> {
    title: &'a str,
    /// What unset means, such as "all"; `None` for a single choice.
    inherit: Option<&'a str>,
    items: Vec<Item>,
    checked: BTreeSet<String>,
    filter: String,
    row: usize,
}

impl List<'_> {
    fn visible(&self) -> Vec<usize> {
        let needle = self.filter.to_lowercase();
        (0..self.items.len())
            .filter(|&i| self.items[i].value.to_lowercase().contains(&needle))
            .collect()
    }

    /// Whether the last row offers to use the filter text as a value.
    fn can_add(&self) -> bool {
        let text = self.filter.trim();
        !text.is_empty() && !self.items.iter().any(|i| i.value == text)
    }

    fn render(&self, ui: &Ui, screen: &Screen) -> Vec<String> {
        let p = &ui.paint;
        let width = screen.width();
        let count = match self.inherit {
            Some(_) => format!("{} checked", self.checked.len()),
            None => format!("{} items", self.items.len()),
        };
        let mut lines = vec![p.header(self.title, &p.muted(&count), width), p.rule(width)];
        lines.push(if self.filter.is_empty() {
            p.dim("  ⌕ type to filter")
        } else {
            format!("  {} {}{}", p.accent("⌕"), self.filter, p.accent("▏"))
        });
        let mut rows: Vec<String> = Vec::new();
        let value_width = self
            .items
            .iter()
            .map(|i| i.value.chars().count())
            .max()
            .unwrap_or(0)
            .clamp(8, 40);
        for i in self.visible() {
            let item = &self.items[i];
            let note = one_line(&item.note, width.saturating_sub(value_width + 12));
            let text = format!("{:<value_width$}  {}", item.value, p.muted(&note));
            rows.push(match self.inherit {
                Some(_) if self.checked.contains(&item.value) => {
                    format!("{} {text}", p.accent("[✓]"))
                }
                Some(_) => format!("{} {text}", p.dim("[ ]")),
                None => text,
            });
        }
        if self.can_add() {
            rows.push(p.accent(&format!("+ use \"{}\"", self.filter.trim())));
        }
        if rows.is_empty() {
            lines.push(p.muted("  Nothing matches."));
        }
        for i in window(self.row, rows.len(), screen.rows().saturating_sub(8).max(5)) {
            lines.push(row_line(ui, &rows[i], i == self.row, width));
        }
        lines.push(p.rule(width));
        lines.push(match self.inherit {
            Some(inherit) => {
                let reset = format!("reset to {inherit}");
                let keys = [
                    ("space", "check"),
                    ("enter", "done"),
                    ("ctrl+a", "all"),
                    ("ctrl+r", &reset),
                    ("esc", "cancel"),
                ];
                hints(p, &keys, width)
            }
            None => hints(
                p,
                &[
                    ("↑↓", "move"),
                    ("enter", "choose"),
                    ("type", "filter"),
                    ("esc", "cancel"),
                ],
                width,
            ),
        });
        lines
    }
}

/// One value, or `None` when the user cancels. Typed text that matches no item can be chosen.
pub async fn pick_one(
    ui: &Ui,
    screen: &mut Screen,
    title: &str,
    items: Vec<Item>,
) -> Result<Option<String>> {
    let list = List {
        title,
        inherit: None,
        items,
        checked: BTreeSet::new(),
        filter: String::new(),
        row: 0,
    };
    Ok(match run(ui, screen, list).await? {
        Some(Picked::List(mut values)) => values.pop(),
        _ => None,
    })
}

/// `None` when the user cancels. `inherit` names what unset means, such as "all".
pub async fn pick(
    ui: &Ui,
    screen: &mut Screen,
    title: &str,
    mut items: Vec<Item>,
    current: Option<&[String]>,
    inherit: &str,
) -> Result<Option<Picked>> {
    let checked: BTreeSet<String> = current.into_iter().flatten().cloned().collect();
    for value in &checked {
        if !items.iter().any(|i| &i.value == value) {
            items.push(Item::new(value.clone(), "added by you"));
        }
    }
    let list = List {
        title,
        inherit: Some(inherit),
        items,
        checked,
        filter: String::new(),
        row: 0,
    };
    run(ui, screen, list).await
}

async fn run(ui: &Ui, screen: &mut Screen, mut list: List<'_>) -> Result<Option<Picked>> {
    let single = list.inherit.is_none();
    loop {
        let visible = list.visible();
        let add = list.can_add();
        let total = visible.len() + usize::from(add);
        list.row = list.row.min(total.saturating_sub(1));
        screen.draw(list.render(ui, screen));
        let Some(key) = screen.next(false).await else {
            if screen.closed {
                return Ok(None);
            }
            continue;
        };
        // The value under the pointer; the last row, "use", holds the typed text.
        let on_add = add && list.row == visible.len();
        let target = match visible.get(list.row) {
            Some(&i) => Some(list.items[i].value.clone()),
            None => on_add.then(|| list.filter.trim().to_owned()),
        };
        match key {
            Key::Up => list.row = list.row.saturating_sub(1),
            Key::Down => list.row = (list.row + 1).min(total.saturating_sub(1)),
            Key::Escape if !list.filter.is_empty() => list.filter.clear(),
            Key::Escape | Key::Ctrl('c') => return Ok(None),
            Key::Enter if single => return Ok(target.map(|value| Picked::List(vec![value]))),
            Key::Ctrl('r') if !single => return Ok(Some(Picked::Inherit)),
            Key::Ctrl('a') if !single => {
                let values: Vec<String> = visible
                    .iter()
                    .map(|&i| list.items[i].value.clone())
                    .collect();
                let all = values.iter().all(|v| list.checked.contains(v));
                for value in values {
                    if all {
                        list.checked.remove(&value);
                    } else {
                        list.checked.insert(value);
                    }
                }
            }
            Key::Char(' ') | Key::Enter if !single && on_add => {
                let value = list.filter.trim().to_owned();
                list.items.push(Item::new(value.clone(), "added by you"));
                list.checked.insert(value);
                list.filter.clear();
            }
            Key::Char(' ') if !single => {
                if let Some(value) = target
                    && !list.checked.remove(&value)
                {
                    list.checked.insert(value);
                }
            }
            Key::Enter => {
                if list.checked.is_empty()
                    && let Some(value) = target
                {
                    list.checked.insert(value);
                }
                let picked = list
                    .items
                    .iter()
                    .filter(|i| list.checked.contains(&i.value))
                    .map(|i| i.value.clone());
                return Ok(Some(Picked::List(picked.collect())));
            }
            Key::Backspace => {
                list.filter.pop();
            }
            Key::Char(c) => list.filter.push(c),
            Key::Text(text) => list.filter.push_str(&text),
            _ => {}
        }
    }
}
