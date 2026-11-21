//! The rows of the Settings tab: options of the global options file, changed in place.

use serde_json::{Value, json};

use crate::config::options::{SWITCHES, Switch};
use crate::config::{Options, ScopeOption};
use crate::model::Source;
use crate::policy::ConflictMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Source(Source),
    LoadScope,
    TrustAll,
    Conflicts,
    /// A feature switch from [`SWITCHES`].
    Switch(&'static Switch),
}

const SCOPES: [ScopeOption; 3] = [ScopeOption::Global, ScopeOption::Project, ScopeOption::Both];

impl Setting {
    /// The Settings tab rows: the base options, then the feature switches.
    pub fn all() -> Vec<Setting> {
        let base = [
            Self::Source(Source::Claude),
            Self::Source(Source::Codex),
            Self::Source(Source::Pi),
            Self::LoadScope,
            Self::TrustAll,
            Self::Conflicts,
        ];
        base.into_iter()
            .chain(SWITCHES.iter().map(Self::Switch))
            .collect()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Source(Source::Claude) => "Claude Code hooks",
            Self::Source(Source::Codex) => "Codex hooks",
            Self::Source(Source::Pi) => "pi hooks",
            Self::LoadScope => "Load hooks from",
            Self::TrustAll => "Trust all hooks",
            Self::Conflicts => "Conflicts between layers",
            Self::Switch(switch) => switch.label,
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Self::Source(Source::Claude) => {
                "Claude Code user, project, local, and managed settings"
            }
            Self::Source(Source::Codex) => {
                "Codex hooks.json and config.toml files, and requirements.toml"
            }
            Self::Source(Source::Pi) => "ext/hooks.json in the agent dir and in the project",
            Self::LoadScope => "The files that load; managed files load in each scope",
            Self::TrustAll => "Run hooks without a trust prompt; denied hooks stay denied",
            Self::Conflicts => "What runs when layers match the same event and matcher",
            Self::Switch(switch) => switch.about,
        }
    }

    /// The key in the global options file.
    pub fn key(self) -> String {
        match self {
            Self::Source(source) => format!("sources/{}", source.name()),
            Self::LoadScope => "scope".into(),
            Self::TrustAll => "trustAll".into(),
            Self::Conflicts => "conflicts".into(),
            Self::Switch(switch) => switch.key.into(),
        }
    }

    /// The values in panel order: the short name and the line in the actions pane.
    pub fn values(self) -> Vec<(&'static str, &'static str)> {
        match self {
            Self::Source(_) => vec![
                ("on", "on: load and run these hooks"),
                ("off", "off: skip these hooks"),
            ],
            Self::TrustAll => vec![
                ("on", "on: run each hook without a trust prompt"),
                ("off", "off: ask before an untrusted hook runs"),
            ],
            Self::LoadScope => vec![
                ("global", "global: only the global files"),
                ("project", "project: only the project files"),
                ("both", "both: the global and the project files"),
            ],
            Self::Conflicts => ConflictMode::ALL
                .iter()
                .map(|mode| (mode.name(), mode.describe()))
                .collect(),
            Self::Switch(_) => vec![("on", "on"), ("off", "off")],
        }
    }

    /// The JSON value that `values()[index]` writes.
    pub fn json(self, index: usize) -> Value {
        match self {
            Self::Source(_) | Self::TrustAll | Self::Switch(_) => json!(index == 0),
            Self::LoadScope => json!(SCOPES[index.min(SCOPES.len() - 1)].name()),
            Self::Conflicts => json!(ConflictMode::ALL[index.min(2)].name()),
        }
    }

    /// The index of the current value.
    pub fn current(self, options: &Options) -> usize {
        let on = |on: bool| usize::from(!on);
        match self {
            Self::Source(source) => on(options.source_on(source)),
            Self::TrustAll => on(options.trust_all),
            Self::Switch(switch) => on(options.switch_on(switch.key)),
            Self::LoadScope => SCOPES.iter().position(|s| *s == options.scope).unwrap_or(2),
            Self::Conflicts => ConflictMode::ALL
                .iter()
                .position(|m| *m == options.conflicts)
                .unwrap_or(0),
        }
    }

    pub fn value_name(self, options: &Options) -> &'static str {
        self.values()[self.current(options)].0
    }

    pub fn matches(self, query: &str, options: &Options) -> bool {
        let fields = [self.label(), self.about(), self.value_name(options)];
        query
            .split_whitespace()
            .all(|word| fields.iter().any(|f| f.to_lowercase().contains(word)))
    }
}
