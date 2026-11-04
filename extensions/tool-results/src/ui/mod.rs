//! The `/tool-results` panel: settings, per-tool rules, and stats.

pub mod render;
pub(crate) mod run;

use pi_ext_sdk::Key;
use serde_json::{Map, Value, json};

use crate::config::{Config, FIELDS, Field, Kind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Settings,
    Rules,
    Stats,
}

pub const TABS: [Tab; 3] = [Tab::Settings, Tab::Rules, Tab::Stats];

#[derive(Clone, Debug, PartialEq)]
pub enum Screen {
    List,
    /// The fields of the rule for one pattern.
    Rule(String),
}

/// A change the panel asks for. `rule` is `None` for top-level keys; `value` `None` removes.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Redraw,
    Close,
    Set {
        rule: Option<String>,
        key: &'static str,
        value: Option<Value>,
    },
    EditNumber {
        rule: Option<String>,
        key: &'static str,
    },
    AddRule,
    RemoveRule(String),
    ResetStats,
}

pub struct View {
    pub tab: usize,
    pub selected: usize,
    pub screen: Screen,
    pub status: Option<String>,
}

impl Default for View {
    fn default() -> Self {
        Self::new()
    }
}

/// The keys the rules for `pattern` set, merged in order, and which come from built-in rules.
pub fn rule_overrides(config: &Config, pattern: &str) -> (Map<String, Value>, Map<String, Value>) {
    let mut user = Map::new();
    let mut builtin = Map::new();
    for rule in config.rules.iter().filter(|r| r.pattern == pattern) {
        let target = if rule.builtin {
            &mut builtin
        } else {
            &mut user
        };
        target.extend(rule.overrides.clone());
    }
    (user, builtin)
}

pub fn rule_fields() -> Vec<&'static Field> {
    FIELDS.iter().filter(|f| !f.global).collect()
}

/// The distinct rule patterns in file order.
pub fn patterns(config: &Config) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for rule in &config.rules {
        if !out.contains(&rule.pattern) {
            out.push(rule.pattern.clone());
        }
    }
    out
}

/// The next value of `field` from `current`; `inherit` adds a `None` step for rules.
fn cycle(
    field: &Field,
    current: Option<&Value>,
    forward: bool,
    inherit: bool,
) -> Option<Option<Value>> {
    let mut steps: Vec<Option<Value>> = match field.kind {
        Kind::Bool => vec![Some(json!(true)), Some(json!(false))],
        Kind::Choice(choices) => choices.iter().map(|c| Some(json!(c))).collect(),
        Kind::Num(_) => return None,
    };
    if inherit {
        steps.insert(0, None);
    }
    let at = steps
        .iter()
        .position(|s| s.as_ref() == current)
        .unwrap_or(0);
    let next = if forward {
        at + 1
    } else {
        at + steps.len() - 1
    };
    Some(steps[next % steps.len()].clone())
}

impl View {
    pub fn new() -> Self {
        Self {
            tab: 0,
            selected: 0,
            screen: Screen::List,
            status: None,
        }
    }

    pub fn tab(&self) -> Tab {
        TABS[self.tab]
    }

    /// Selectable rows of the current screen.
    pub fn len(&self, config: &Config, tools: usize) -> usize {
        match (&self.screen, self.tab()) {
            (Screen::Rule(_), _) => rule_fields().len(),
            (Screen::List, Tab::Settings) => FIELDS.len(),
            (Screen::List, Tab::Rules) => patterns(config).len() + 1,
            (Screen::List, Tab::Stats) => tools.max(1),
        }
    }

    pub fn clamp(&mut self, config: &Config, tools: usize) {
        self.selected = self.selected.min(self.len(config, tools).saturating_sub(1));
    }

    pub fn key(&mut self, key: &Key, config: &Config, tools: usize) -> Action {
        self.status = None;
        let len = self.len(config, tools);
        let in_rule = matches!(self.screen, Screen::Rule(_));
        match key {
            Key::Escape | Key::Ctrl('c') | Key::Char('q') if in_rule => {
                let pattern = match &self.screen {
                    Screen::Rule(p) => p.clone(),
                    Screen::List => String::new(),
                };
                self.screen = Screen::List;
                self.selected = patterns(config)
                    .iter()
                    .position(|p| *p == pattern)
                    .unwrap_or(0);
                return Action::Redraw;
            }
            _ if key.is_cancel() => return Action::Close,
            Key::Tab | Key::BackTab if !in_rule => {
                let step = if *key == Key::Tab { 1 } else { TABS.len() - 1 };
                self.tab = (self.tab + step) % TABS.len();
                self.selected = 0;
                return Action::Redraw;
            }
            Key::Up => self.selected = (self.selected + len - 1) % len.max(1),
            Key::Down => self.selected = (self.selected + 1) % len.max(1),
            Key::Home => self.selected = 0,
            Key::End => self.selected = len.saturating_sub(1),
            Key::PageUp => self.selected = self.selected.saturating_sub(10),
            Key::PageDown => self.selected = (self.selected + 10).min(len.saturating_sub(1)),
            _ => return self.act(key, config),
        }
        Action::Redraw
    }

    fn act(&mut self, key: &Key, config: &Config) -> Action {
        let forward = !matches!(key, Key::Left);
        let change = matches!(key, Key::Enter | Key::Left | Key::Right | Key::Char(' '));
        match (self.screen.clone(), self.tab()) {
            (Screen::Rule(pattern), _) if change => {
                let field = rule_fields()[self.selected.min(rule_fields().len() - 1)];
                let (user, builtin) = rule_overrides(config, &pattern);
                let current = user.get(field.key).or(builtin.get(field.key));
                match cycle(field, current, forward, true) {
                    Some(value) => Action::Set {
                        rule: Some(pattern),
                        key: field.key,
                        value,
                    },
                    None if *key == Key::Enter => Action::EditNumber {
                        rule: Some(pattern),
                        key: field.key,
                    },
                    None => Action::Redraw,
                }
            }
            (Screen::List, Tab::Settings) if change => {
                let field = &FIELDS[self.selected.min(FIELDS.len() - 1)];
                let current = config.get(field.key);
                match cycle(field, Some(&current), forward, false) {
                    Some(value) => Action::Set {
                        rule: None,
                        key: field.key,
                        value,
                    },
                    None if *key == Key::Enter => Action::EditNumber {
                        rule: None,
                        key: field.key,
                    },
                    None => Action::Redraw,
                }
            }
            (Screen::List, Tab::Rules) => {
                let list = patterns(config);
                let picked = list.get(self.selected).cloned();
                match (key, picked) {
                    (Key::Char('a'), _) | (Key::Enter, None) => Action::AddRule,
                    (Key::Enter | Key::Right, Some(pattern)) => {
                        self.screen = Screen::Rule(pattern);
                        self.selected = 0;
                        Action::Redraw
                    }
                    (Key::Char('d') | Key::Delete, Some(pattern)) => {
                        let user = config
                            .rules
                            .iter()
                            .any(|r| r.pattern == pattern && !r.builtin);
                        if user {
                            Action::RemoveRule(pattern)
                        } else {
                            self.status = Some(format!(
                                "{pattern} is a built-in rule; turn off \"Built-in rule for read\" in Settings."
                            ));
                            Action::Redraw
                        }
                    }
                    _ => Action::Redraw,
                }
            }
            (Screen::List, Tab::Stats) if matches!(key, Key::Char('r')) => Action::ResetStats,
            _ => Action::Redraw,
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
