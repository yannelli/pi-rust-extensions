//! The state of the `/hooks` panel and what its keys ask the command loop to do.

use super::{Row, Setting, Tab};
use crate::config::Options;
use crate::model::Scope;

/// One selectable line of the current tab.
#[derive(Debug, Clone, Copy)]
pub enum Entry<'a> {
    /// The "Add a hook" row, first in the hook tabs.
    Add,
    Hook(&'a Row),
    Setting(Setting),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustChange {
    Always,
    Once,
    Untrust,
    Deny,
}

/// Actions of a hook in the actions pane, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailAction {
    Toggle,
    TrustAlways,
    AllowOnce,
    Untrust,
    Deny,
    Remove,
}

impl DetailAction {
    pub const ALL: [DetailAction; 6] = [
        DetailAction::Toggle,
        DetailAction::TrustAlways,
        DetailAction::AllowOnce,
        DetailAction::Untrust,
        DetailAction::Deny,
        DetailAction::Remove,
    ];

    pub fn label(self, enabled: bool) -> &'static str {
        match self {
            DetailAction::Toggle if enabled => "Disable",
            DetailAction::Toggle => "Enable",
            DetailAction::TrustAlways => "Trust always",
            DetailAction::AllowOnce => "Allow once this session",
            DetailAction::Untrust => "Untrust",
            DetailAction::Deny => "Deny",
            DetailAction::Remove => "Remove from file",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddStep {
    Event,
    Matcher,
    Command,
}

/// The "Add a hook" form: pick the event, then type the matcher and the command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddForm {
    pub step: AddStep,
    pub event: usize,
    pub matcher: String,
    pub command: String,
    pub scope: Scope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    List,
    /// Details and actions of the selected hook or setting.
    Menu {
        choice: usize,
    },
    /// Removes a hook from its file after a second enter.
    Confirm {
        hash: String,
    },
    Add(AddForm),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Redraw,
    Close,
    Toggle(String),
    Remove(String),
    Trust(String, TrustChange),
    Add {
        scope: Scope,
        event: String,
        matcher: String,
        command: String,
    },
    /// Writes `values()[index]` of the setting to the global options file.
    Set(Setting, usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Warn,
}

#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub tab: Tab,
    pub selected: usize,
    pub query: String,
    pub screen: Screen,
    pub status: Option<(Tone, String)>,
    pub project_trusted: bool,
    pub options: Options,
    /// `--hooks-trust-all` is set for this run.
    pub trust_flag: bool,
}

impl View {
    pub fn new(project_trusted: bool) -> Self {
        Self {
            tab: Tab::All,
            selected: 0,
            query: String::new(),
            screen: Screen::List,
            status: None,
            project_trusted,
            options: Options::default(),
            trust_flag: false,
        }
    }

    pub fn trust_all(&self) -> bool {
        self.trust_flag || self.options.trust_all
    }

    /// The scopes a new hook can go to.
    pub fn scopes(&self) -> Vec<Scope> {
        let mut scopes = vec![Scope::Global];
        if self.project_trusted {
            scopes.push(Scope::Project);
        }
        scopes
    }

    pub fn entries<'a>(&self, rows: &'a [Row]) -> Vec<Entry<'a>> {
        let query = self.query.to_lowercase();
        if self.tab == Tab::Settings {
            return Setting::all()
                .into_iter()
                .filter(|s| s.matches(&query, &self.options))
                .map(Entry::Setting)
                .collect();
        }
        let mut entries = Vec::new();
        if query.is_empty() {
            entries.push(Entry::Add);
        }
        entries.extend(
            rows.iter()
                .filter(|r| self.tab.shows(r) && r.matches(&query))
                .map(Entry::Hook),
        );
        entries
    }

    pub fn selected_entry<'a>(&self, rows: &'a [Row]) -> Option<Entry<'a>> {
        self.entries(rows).get(self.selected).copied()
    }

    /// Moves the cursor to the row with `hash` when the current tab shows it.
    pub fn select_hash(&mut self, rows: &[Row], hash: &str) {
        let at = self
            .entries(rows)
            .iter()
            .position(|e| matches!(e, Entry::Hook(row) if row.def.hash == hash));
        if let Some(at) = at {
            self.selected = at;
        }
    }

    /// Keeps the cursor and screen valid after rows change.
    pub fn clamp(&mut self, rows: &[Row]) {
        let len = self.entries(rows).len();
        self.selected = self.selected.min(len.saturating_sub(1));
        let gone = match &self.screen {
            Screen::Confirm { hash } => !rows.iter().any(|r| &r.def.hash == hash),
            Screen::Menu { .. } => self.selected_entry(rows).is_none(),
            _ => false,
        };
        if gone {
            self.screen = Screen::List;
        }
    }

    /// The number of actions in the actions pane of the selected entry.
    pub fn choice_count(&self, rows: &[Row]) -> usize {
        match self.selected_entry(rows) {
            Some(Entry::Hook(row)) if !row.managed() => DetailAction::ALL.len(),
            Some(Entry::Setting(setting)) => setting.values().len(),
            _ => 0,
        }
    }

    pub(super) fn add_form(&self) -> AddForm {
        let scope = match self.tab {
            Tab::Project if self.project_trusted => Scope::Project,
            _ => Scope::Global,
        };
        AddForm {
            step: AddStep::Event,
            event: 0,
            matcher: String::new(),
            command: String::new(),
            scope,
        }
    }
}
