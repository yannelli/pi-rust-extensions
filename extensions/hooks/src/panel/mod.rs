//! `/hooks` manager panel, laid out like `/deferred` and `/claude-plugins`: tabs, a search line,
//! a list, and an actions pane. State in `view.rs`, keys in `keys.rs`, lines in `render.rs`.

mod details;
mod keys;
mod list;
mod render;
mod settings;
mod view;

pub use details::run_text;
pub use settings::Setting;
pub use view::{Action, AddForm, AddStep, DetailAction, Entry, Screen, Tone, TrustChange, View};

use crate::model::{HookDef, Layer, Scope, Source};
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
    /// Other layers that define different hooks for the same event and matcher.
    pub conflict: Option<String>,
    /// Under `conflicts: "override"`, a more specific layer replaces this hook.
    pub shadowed: bool,
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
            conflict: None,
            shadowed: false,
        }
    }

    /// `shadowed`, `conflict`, or `None`.
    pub fn tag(&self) -> Option<&'static str> {
        if self.shadowed {
            Some("shadowed")
        } else {
            self.conflict.as_ref().map(|_| "conflict")
        }
    }

    /// `global`, `project`, `local`, or `managed`.
    pub fn scope_label(&self) -> &'static str {
        self.def.layer.name()
    }

    pub fn matcher(&self) -> &str {
        self.def
            .matcher
            .as_deref()
            .filter(|m| !m.is_empty())
            .unwrap_or("*")
    }

    pub fn managed(&self) -> bool {
        self.def.layer == Layer::Managed
    }

    fn matches(&self, query: &str) -> bool {
        let plugin = self.def.plugin.as_ref().and_then(|p| p.id.as_deref());
        let summary = self.def.summary();
        let file = self.def.file.to_string_lossy();
        let fields = [
            self.def.event.as_str(),
            self.matcher(),
            &summary,
            self.group.title(),
            self.def.source.name(),
            self.scope_label(),
            &file,
            plugin.unwrap_or_default(),
            self.tag().unwrap_or_default(),
        ];
        query
            .split_whitespace()
            .all(|word| fields.iter().any(|f| f.to_lowercase().contains(word)))
    }
}

/// Sorts rows by group, event, layer, and matcher.
pub fn sort_rows(rows: &mut [Row]) {
    rows.sort_by(|a, b| {
        (a.group, &a.def.event, a.def.layer, a.matcher()).cmp(&(
            b.group,
            &b.def.event,
            b.def.layer,
            b.matcher(),
        ))
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    All,
    Project,
    Global,
    Settings,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::All, Tab::Project, Tab::Global, Tab::Settings];

    pub fn title(self) -> &'static str {
        match self {
            Tab::All => "All",
            Tab::Project => "Project",
            Tab::Global => "Global",
            Tab::Settings => "Settings",
        }
    }

    fn shows(self, row: &Row) -> bool {
        match self {
            Tab::All => true,
            Tab::Project => row.def.scope == Scope::Project,
            Tab::Global => row.def.scope == Scope::Global,
            Tab::Settings => false,
        }
    }
}

#[cfg(test)]
mod height_tests;
#[cfg(test)]
mod tests;
