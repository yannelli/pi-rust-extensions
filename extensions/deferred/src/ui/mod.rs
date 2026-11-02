//! The `/deferred` panel, laid out like pi's `/acp` browser: tabs, a search line, a list,
//! and an actions pane.

mod keys;
mod menu;
pub mod render;
pub mod rows;
pub(crate) mod run;

use crate::config::{Kind, Mode};
use rows::{Inventory, Row, RuleRow, Scope};

pub const MODES: [Mode; 4] = [Mode::On, Mode::Names, Mode::Deferred, Mode::Off];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Kind(Kind),
    Rules,
}

pub const TABS: [Tab; 5] = [
    Tab::Kind(Kind::Tool),
    Tab::Kind(Kind::Skill),
    Tab::Kind(Kind::Context),
    Tab::Kind(Kind::Section),
    Tab::Rules,
];

/// A rule edit: `mode` `None` removes the rule.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub kind: Kind,
    pub pattern: String,
    pub mode: Option<Mode>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    Set(Mode),
    Remove,
    Unload,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Screen {
    List,
    Menu { choice: usize },
    Scope { change: Change, choice: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Redraw,
    Close,
    Write { change: Change, scope: Scope },
    Unload(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Info,
    Busy,
    Warn,
}

/// One selectable line of the current tab.
pub enum Entry<'a> {
    Item(&'a Row),
    Rule(&'a RuleRow),
}

pub struct View {
    pub tab: usize,
    pub query: String,
    pub selected: usize,
    pub screen: Screen,
    pub status: Option<(Tone, String)>,
    pub trusted: bool,
}

impl View {
    pub fn new(trusted: bool) -> Self {
        Self {
            tab: 0,
            query: String::new(),
            selected: 0,
            screen: Screen::List,
            status: None,
            trusted,
        }
    }

    pub fn tab(&self) -> Tab {
        TABS[self.tab]
    }

    fn matches(&self, text: &[&str]) -> bool {
        let query = self.query.to_lowercase();
        query
            .split_whitespace()
            .all(|word| text.iter().any(|t| t.to_lowercase().contains(word)))
    }

    pub fn entries<'a>(&self, inv: &'a Inventory) -> Vec<Entry<'a>> {
        match self.tab() {
            Tab::Kind(kind) => inv
                .rows
                .iter()
                .filter(|r| {
                    r.kind == kind && self.matches(&[&r.name, &r.description, r.mode.as_str()])
                })
                .map(Entry::Item)
                .collect(),
            Tab::Rules => inv
                .rules
                .iter()
                .filter(|r| self.matches(&[&r.pattern, r.kind.key(), r.mode.as_str(), &r.source]))
                .map(Entry::Rule)
                .collect(),
        }
    }

    pub fn clamp(&mut self, inv: &Inventory) {
        let len = self.entries(inv).len();
        self.selected = self.selected.min(len.saturating_sub(1));
    }

    /// The actions of the selected entry; empty when it cannot be edited here.
    pub fn choices(&self, inv: &Inventory) -> Vec<Choice> {
        let mut choices: Vec<Choice> = MODES.iter().map(|m| Choice::Set(*m)).collect();
        match self.entries(inv).get(self.selected) {
            Some(Entry::Item(row)) if !row.pinned() => {
                if row.exact_scope().is_some() {
                    choices.push(Choice::Remove);
                }
                if row.loaded {
                    choices.push(Choice::Unload);
                }
                choices
            }
            Some(Entry::Rule(rule)) if rule.scope.is_some() => {
                choices.push(Choice::Remove);
                choices
            }
            _ => Vec::new(),
        }
    }

    /// The edit a choice makes, and the scope when it is already known.
    pub(super) fn change(
        &self,
        inv: &Inventory,
        choice: &Choice,
    ) -> Option<(Change, Option<Scope>)> {
        let mode = match choice {
            Choice::Set(mode) => Some(*mode),
            Choice::Remove => None,
            Choice::Unload => return None,
        };
        match self.entries(inv).get(self.selected)? {
            Entry::Item(row) => Some((
                Change {
                    kind: row.kind,
                    pattern: row.name.clone(),
                    mode,
                },
                row.exact_scope(),
            )),
            Entry::Rule(rule) => Some((
                Change {
                    kind: rule.kind,
                    pattern: rule.pattern.clone(),
                    mode,
                },
                rule.scope,
            )),
        }
    }

    /// Write at the known scope, ask when the project is trusted, else write globally.
    pub(super) fn write(&mut self, change: Change, scope: Option<Scope>) -> Action {
        match (scope, self.trusted) {
            (Some(scope), _) => Action::Write { change, scope },
            (None, true) => {
                self.screen = Screen::Scope { change, choice: 0 };
                Action::Redraw
            }
            (None, false) => Action::Write {
                change,
                scope: Scope::Global,
            },
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
