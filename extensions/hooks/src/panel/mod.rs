//! `/hooks` manager panel: rows, view state, and actions. Keys in `keys.rs`, lines in `render.rs`.

mod details;
mod keys;
mod render;

pub use details::run_text;

use crate::model::{HookDef, Scope, Source};
use crate::trust::{LastRun, Trust};

pub const PAGE: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Claude,
    Codex,
    Pi,
    Plugins,
}

impl Group {
    pub fn of(def: &HookDef) -> Self {
        match def.source {
            Source::Claude => Group::Claude,
            Source::Codex => Group::Codex,
            Source::Pi if def.plugin.is_some() => Group::Plugins,
            Source::Pi => Group::Pi,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Group::Claude => "Claude Code",
            Group::Codex => "Codex",
            Group::Pi => "pi",
            Group::Plugins => "Plugins",
        }
    }
}

/// One hook handler with the state the panel shows.
#[derive(Debug, Clone)]
pub struct Row {
    pub def: HookDef,
    pub group: Group,
    pub enabled: bool,
    pub trust: Trust,
    pub last: Option<LastRun>,
}

impl Row {
    pub fn new(def: HookDef, enabled: bool, trust: Trust, last: Option<LastRun>) -> Self {
        let group = Group::of(&def);
        Self {
            def,
            group,
            enabled,
            trust,
            last,
        }
    }

    /// `global`, `project`, or `local` for `.claude/settings.local.json`.
    pub fn scope_label(&self) -> &'static str {
        let local = self
            .def
            .file
            .file_name()
            .is_some_and(|n| n == "settings.local.json");
        match self.def.scope {
            Scope::Project if local => "local",
            scope => scope.name(),
        }
    }

    pub fn matcher(&self) -> &str {
        self.def
            .matcher
            .as_deref()
            .filter(|m| !m.is_empty())
            .unwrap_or("*")
    }

    fn matches(&self, query: &str) -> bool {
        let plugin = self.def.plugin.as_ref().and_then(|p| p.id.as_deref());
        let fields = [
            self.def.event.as_str(),
            self.matcher(),
            &self.def.summary(),
            self.group.title(),
            self.def.source.name(),
            self.scope_label(),
            &self.def.file.to_string_lossy(),
            plugin.unwrap_or_default(),
        ];
        fields.iter().any(|f| f.to_lowercase().contains(query))
    }
}

/// Sorts rows by group, event, scope, and matcher.
pub fn sort_rows(rows: &mut [Row]) {
    rows.sort_by(|a, b| {
        (
            a.group,
            &a.def.event,
            a.def.scope,
            a.scope_label(),
            a.matcher(),
        )
            .cmp(&(
                b.group,
                &b.def.event,
                b.def.scope,
                b.scope_label(),
                b.matcher(),
            ))
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    All,
    Project,
    Global,
}

impl Tab {
    const ALL: [Tab; 3] = [Tab::All, Tab::Project, Tab::Global];

    pub fn title(self) -> &'static str {
        match self {
            Tab::All => "All",
            Tab::Project => "Project",
            Tab::Global => "Global",
        }
    }

    fn shows(self, row: &Row) -> bool {
        match self {
            Tab::All => true,
            Tab::Project => row.def.scope == Scope::Project,
            Tab::Global => row.def.scope == Scope::Global,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    List,
    Search,
    Details { hash: String, action: usize },
    ConfirmRemove { hash: String },
    AddScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustChange {
    Always,
    Once,
    Untrust,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Redraw,
    Close,
    Toggle(String),
    Remove(String),
    Trust(String, TrustChange),
    Add(Scope),
    TrustAll,
    Sources,
    LoadScope,
}

/// Actions in the details view, in display order.
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub tab: Tab,
    pub selected: usize,
    pub query: String,
    pub mode: Mode,
    pub status: Option<String>,
    pub project_trusted: bool,
    pub trust_all: bool,
}

impl View {
    pub fn new(project_trusted: bool) -> Self {
        Self {
            tab: Tab::All,
            selected: 0,
            query: String::new(),
            mode: Mode::List,
            status: None,
            project_trusted,
            trust_all: false,
        }
    }

    /// Indices into `rows` that the tab and search show.
    pub fn visible(&self, rows: &[Row]) -> Vec<usize> {
        let query = self.query.to_lowercase();
        (0..rows.len())
            .filter(|&i| self.tab.shows(&rows[i]) && rows[i].matches(&query))
            .collect()
    }

    pub fn selected_row<'a>(&self, rows: &'a [Row]) -> Option<&'a Row> {
        let visible = self.visible(rows);
        let index = *visible.get(self.selected.min(visible.len().checked_sub(1)?))?;
        rows.get(index)
    }

    /// Moves the cursor to the row with `hash` when it is visible.
    pub fn select_hash(&mut self, rows: &[Row], hash: &str) {
        let visible = self.visible(rows);
        if let Some(position) = visible.iter().position(|&i| rows[i].def.hash == hash) {
            self.selected = position;
        }
    }

    /// Keeps the cursor and mode valid after rows change.
    pub fn clamp(&mut self, rows: &[Row]) {
        self.selected = self
            .selected
            .min(self.visible(rows).len().saturating_sub(1));
        let gone = |hash: &str| !rows.iter().any(|r| r.def.hash == hash);
        match &self.mode {
            Mode::Details { hash, .. } | Mode::ConfirmRemove { hash } if gone(hash) => {
                self.mode = Mode::List;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod height_tests;
#[cfg(test)]
mod tests;
