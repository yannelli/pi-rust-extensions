//! The `/claude-plugins` panel, laid out like `/deferred`: tabs (installed plugins, plugins to
//! discover, marketplaces), a search line, a list, and a details-and-actions pane. The panel
//! stays open: toggles apply inline, slow git work shows progress in the status line, and adding
//! a marketplace is an input line in the panel.

pub mod keys;
pub mod options;
pub mod render;
pub mod run;

#[cfg(test)]
mod tests;

use crate::env::{Env, Scope};
use crate::listing::{self, PluginRow, Status};
use crate::state::State;

#[derive(Clone, Debug)]
pub struct Plugin {
    pub row: PluginRow,
    /// Staged prompt commands, skills, and agents of an installed plugin.
    pub commands: usize,
    pub skills: usize,
    pub agents: usize,
    pub hooks: bool,
    pub mcp: Vec<String>,
}

impl Plugin {
    pub fn id(&self) -> String {
        self.row.id()
    }

    pub fn installed(&self) -> bool {
        self.row.enabled.is_some()
    }

    pub fn enabled(&self) -> bool {
        self.row.enabled == Some(true)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Market {
    pub name: String,
    pub scope: Scope,
    pub source: String,
    pub plugins: usize,
    pub installed: usize,
    pub auto_update: bool,
    pub git: bool,
}

/// Everything the panel shows, read on a blocking thread.
#[derive(Clone, Debug, Default)]
pub struct Inventory {
    pub plugins: Vec<Plugin>,
    pub markets: Vec<Market>,
    pub error: Option<String>,
}

impl Inventory {
    pub fn read(env: &Env) -> Self {
        match Self::try_read(env) {
            Ok(inventory) => inventory,
            Err(err) => Self {
                error: Some(format!("{err:#}")),
                ..Self::default()
            },
        }
    }

    fn try_read(env: &Env) -> anyhow::Result<Self> {
        let mut plugins = Vec::new();
        let mut states = Vec::new();
        for scope in env.visible_scopes() {
            states.push((scope, State::load(&env.dirs(scope))?));
        }
        for market in listing::plugins(env, None, None)? {
            for row in market.plugins {
                let record = states
                    .iter()
                    .filter(|(scope, _)| row.enabled.is_none() || *scope == row.scope)
                    .find_map(|(_, state)| state.plugins.get(&row.id()));
                let count = |dir: &str| {
                    record.map_or(0, |r| {
                        let parent = |f: &std::path::PathBuf| {
                            f.parent()
                                .and_then(|p| p.file_name())
                                .and_then(|n| n.to_str())
                                .map(str::to_owned)
                        };
                        r.files
                            .iter()
                            .filter(|f| parent(f).as_deref() == Some(dir))
                            .count()
                    })
                };
                plugins.push(Plugin {
                    commands: count("prompts"),
                    skills: count("skills"),
                    agents: count("agents"),
                    hooks: record.is_some_and(|r| r.hooks),
                    mcp: record.map(|r| r.mcp_keys.clone()).unwrap_or_default(),
                    row,
                });
            }
        }
        let mut markets = Vec::new();
        for located in crate::market::all(env)? {
            let config = crate::config::Config::load(&env.dirs(located.scope))?;
            let auto_update = config
                .marketplaces()
                .into_iter()
                .any(|(name, _, auto)| name == located.record.name && auto);
            let name = located.record.name.clone();
            let in_market = plugins.iter().filter(|p| p.row.marketplace == name);
            markets.push(Market {
                plugins: in_market.clone().count(),
                installed: in_market.filter(|p| p.installed()).count(),
                scope: located.scope,
                source: located.record.source.clone(),
                git: located.record.git_url.is_some(),
                auto_update,
                name,
            });
        }
        Ok(Self {
            plugins,
            markets,
            error: None,
        })
    }

    pub fn installed(&self) -> usize {
        self.plugins.iter().filter(|p| p.installed()).count()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Installed,
    Discover,
    Marketplaces,
}

pub const TABS: [Tab; 3] = [Tab::Installed, Tab::Discover, Tab::Marketplaces];

/// The rows of the marketplaces tab before the marketplaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Special {
    Add,
    UpdateAll,
    Import,
}

pub const SPECIALS: [Special; 3] = [Special::Add, Special::UpdateAll, Special::Import];

/// One selectable line of the current tab.
#[derive(Clone, Copy, Debug)]
pub enum Entry<'a> {
    Plugin(&'a Plugin),
    Market(&'a Market),
    Special(Special),
}

/// An operation on a blocking thread; the panel shows progress while it runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Install {
        id: String,
        scope: Scope,
    },
    Reinstall {
        id: String,
        scope: Scope,
    },
    Update {
        id: String,
        scope: Scope,
    },
    Uninstall {
        id: String,
        scope: Scope,
    },
    SetEnabled {
        id: String,
        scope: Scope,
        enabled: bool,
    },
    Configure {
        id: String,
    },
    AddMarket {
        source: String,
        scope: Scope,
    },
    UpdateMarket {
        name: String,
        scope: Scope,
    },
    RemoveMarket {
        name: String,
        scope: Scope,
    },
    AutoUpdate {
        name: String,
        on: bool,
    },
    UpdateAll,
    Import,
}

impl Op {
    /// The status line while it runs.
    pub fn label(&self) -> String {
        match self {
            Op::Install { id, scope } => format!("Installing {id} ({})", scope_word(*scope)),
            Op::Reinstall { id, .. } => format!("Reinstalling {id}"),
            Op::Update { id, .. } => format!("Updating {id}"),
            Op::Uninstall { id, .. } => format!("Uninstalling {id}"),
            Op::SetEnabled {
                id, enabled: true, ..
            } => format!("Turning on {id}"),
            Op::SetEnabled { id, .. } => format!("Turning off {id}"),
            Op::Configure { id } => format!("Configuring {id}"),
            Op::AddMarket { source, .. } => format!("Adding {source} (cloning)"),
            Op::UpdateMarket { name, .. } => format!("Updating {name} (pulling)"),
            Op::RemoveMarket { name, .. } => format!("Removing {name}"),
            Op::AutoUpdate { name, on: true } => format!("Turning on auto-update of {name}"),
            Op::AutoUpdate { name, .. } => format!("Turning off auto-update of {name}"),
            Op::UpdateAll => "Updating every marketplace and plugin".into(),
            Op::Import => "Importing from Claude Code".into(),
        }
    }

    /// Whether the plugin may have options to ask for afterwards.
    pub fn configures(&self) -> Option<&str> {
        match self {
            Op::Install { id, .. }
            | Op::SetEnabled {
                id, enabled: true, ..
            } => Some(id),
            _ => None,
        }
    }
}

pub fn scope_word(scope: Scope) -> &'static str {
    match scope {
        Scope::User => "every project",
        Scope::Project => "this project",
    }
}

/// A choice of the actions pane.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub label: String,
    pub op: Option<Op>,
    /// Asks for a second Enter first.
    pub confirm: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Screen {
    List,
    Menu { choice: usize },
    Confirm { op: Op, question: String },
    Add { source: String, scope: Scope },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Redraw,
    Close,
    Run(Op),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Info,
    Busy,
    Warn,
}

pub struct View {
    pub tab: usize,
    pub query: String,
    pub selected: usize,
    pub screen: Screen,
    pub status: Option<(Tone, String)>,
    pub trusted: bool,
    /// Frame of the progress spinner while an operation runs.
    pub frame: usize,
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
            frame: 0,
        }
    }

    pub fn tab(&self) -> Tab {
        TABS[self.tab]
    }

    fn matches(&self, text: &str) -> bool {
        let text = text.to_lowercase();
        self.query
            .to_lowercase()
            .split_whitespace()
            .all(|word| text.contains(word))
    }

    pub fn entries<'a>(&self, inv: &'a Inventory) -> Vec<Entry<'a>> {
        let plugin_text = |p: &Plugin| {
            format!(
                "{} {} {}",
                p.id(),
                p.row.description.as_deref().unwrap_or(""),
                p.row.reasons.join(" ")
            )
        };
        match self.tab() {
            Tab::Installed => inv
                .plugins
                .iter()
                .filter(|p| p.installed() && self.matches(&plugin_text(p)))
                .map(Entry::Plugin)
                .collect(),
            Tab::Discover => {
                let mut rows: Vec<&Plugin> = inv
                    .plugins
                    .iter()
                    .filter(|p| !p.installed() && self.matches(&plugin_text(p)))
                    .collect();
                // Installable plugins first.
                rows.sort_by_key(|p| p.row.status == Status::Unavailable);
                rows.into_iter().map(Entry::Plugin).collect()
            }
            Tab::Marketplaces => {
                let mut out: Vec<Entry<'a>> = Vec::new();
                if self.query.is_empty() {
                    out.extend(SPECIALS.iter().map(|s| Entry::Special(*s)));
                }
                out.extend(
                    inv.markets
                        .iter()
                        .filter(|m| self.matches(&format!("{} {}", m.name, m.source)))
                        .map(Entry::Market),
                );
                out
            }
        }
    }

    pub fn count(&self, inv: &Inventory, tab: Tab) -> usize {
        match tab {
            Tab::Installed => inv.installed(),
            Tab::Discover => inv.plugins.len() - inv.installed(),
            Tab::Marketplaces => inv.markets.len(),
        }
    }

    /// The scopes a plugin can be installed in, or a marketplace added to.
    pub fn scopes(&self) -> Vec<Scope> {
        if self.trusted {
            vec![Scope::User, Scope::Project]
        } else {
            vec![Scope::User]
        }
    }

    /// The actions pane of the selected entry.
    pub fn choices(&self, inv: &Inventory) -> Vec<Choice> {
        let choice = |label: String, op: Op| Choice {
            label,
            op: Some(op),
            confirm: false,
        };
        let confirm = |label: String, op: Op| Choice {
            label,
            op: Some(op),
            confirm: true,
        };
        let mut out = Vec::new();
        match self.entries(inv).get(self.selected) {
            Some(Entry::Plugin(p)) if p.installed() => {
                let (id, scope) = (p.id(), p.row.scope);
                out.push(choice(
                    if p.enabled() {
                        "Turn off".into()
                    } else {
                        "Turn on".into()
                    },
                    Op::SetEnabled {
                        id: id.clone(),
                        scope,
                        enabled: !p.enabled(),
                    },
                ));
                if p.enabled() {
                    out.push(choice(
                        "Update".into(),
                        Op::Update {
                            id: id.clone(),
                            scope,
                        },
                    ));
                    out.push(choice("Configure".into(), Op::Configure { id: id.clone() }));
                }
                out.push(choice(
                    "Reinstall".into(),
                    Op::Reinstall {
                        id: id.clone(),
                        scope,
                    },
                ));
                out.push(confirm("Uninstall".into(), Op::Uninstall { id, scope }));
            }
            Some(Entry::Plugin(p)) if p.row.status != Status::Unavailable => {
                for scope in self.scopes() {
                    out.push(choice(
                        format!("Install for {}", scope_word(scope)),
                        Op::Install { id: p.id(), scope },
                    ));
                }
            }
            Some(Entry::Market(m)) => {
                let (name, scope) = (m.name.clone(), m.scope);
                out.push(choice(
                    if m.git {
                        "Update now (git pull)".into()
                    } else {
                        "Read again".into()
                    },
                    Op::UpdateMarket {
                        name: name.clone(),
                        scope,
                    },
                ));
                if m.git {
                    out.push(choice(
                        if m.auto_update {
                            "Turn off auto-update".into()
                        } else {
                            "Turn on auto-update (at session start)".into()
                        },
                        Op::AutoUpdate {
                            name: name.clone(),
                            on: !m.auto_update,
                        },
                    ));
                }
                out.push(confirm(
                    "Remove marketplace and its plugins".into(),
                    Op::RemoveMarket { name, scope },
                ));
            }
            _ => {}
        }
        out
    }
}
