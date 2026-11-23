//! `/iterm2` panel, laid out like `/hooks`: tabs, a list, an actions pane, and a hex field.
//! Keys in `keys.rs`, lines in `render.rs`.

mod keys;
mod render;

use serde_json::{Map, Value, json};

use crate::features::Config;
use crate::sequences::{ProgramState, state_name};
use crate::themes::{COLORED, Choice, Theme, hex, parse_hex};

/// A feature switch of the config, in panel order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    Enabled,
    TabColor,
    Badge,
    UserVars,
    Attention,
    Cwd,
}

impl Feature {
    pub const ALL: [Feature; 6] = [
        Feature::Enabled,
        Feature::TabColor,
        Feature::Badge,
        Feature::UserVars,
        Feature::Attention,
        Feature::Cwd,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Feature::Enabled => "enabled",
            Feature::TabColor => "tabColor",
            Feature::Badge => "badge",
            Feature::UserVars => "userVars",
            Feature::Attention => "attention",
            Feature::Cwd => "cwd",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Feature::Enabled => "Integration",
            Feature::TabColor => "Tab color",
            Feature::Badge => "Badge",
            Feature::UserVars => "Status bar variables",
            Feature::Attention => "Attention",
            Feature::Cwd => "Current directory",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Feature::Enabled => "Off turns every feature off",
            Feature::TabColor => "Color the tab by agent state",
            Feature::Badge => "Name the session in the pane badge",
            Feature::UserVars => "Set user.pi_state, pi_session, pi_project, and pi_model",
            Feature::Attention => "Request attention when a long run ends",
            Feature::Cwd => "Tell iTerm2 the working directory",
        }
    }

    /// The saved switch, apart from `enabled`; a switch that is not named is on.
    pub fn on(self, config: &Map<String, Value>) -> bool {
        config
            .get(self.key())
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Colors,
    Features,
}

impl Tab {
    pub const ALL: [Tab; 2] = [Tab::Colors, Tab::Features];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Colors => "Colors",
            Tab::Features => "Features",
        }
    }

    pub fn rows(self) -> Vec<Row> {
        match self {
            Tab::Colors => std::iter::once(Row::Theme)
                .chain(COLORED.map(Row::State))
                .chain([Row::Reset])
                .collect(),
            Tab::Features => Feature::ALL.map(Row::Feature).to_vec(),
        }
    }
}

/// One selectable line of a tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Theme,
    State(ProgramState),
    Reset,
    Feature(Feature),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    List,
    /// The values of the selected row.
    Menu {
        choice: usize,
    },
    /// A custom `#rrggbb` color for a state.
    Hex {
        state: ProgramState,
        text: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Info,
    Warn,
}

/// A change to the global config file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Theme(Theme),
    Color(ProgramState, Choice),
    ResetColors,
    Feature(Feature, bool),
}

impl Edit {
    pub fn apply(self, config: &mut Map<String, Value>) {
        match self {
            Edit::Theme(theme) => {
                config.insert("theme".into(), json!(theme.name()));
            }
            Edit::Color(state, choice) => {
                let mut colors = match config.remove("colors") {
                    Some(Value::Object(colors)) => colors,
                    _ => Map::new(),
                };
                match choice.json() {
                    Some(value) => colors.insert(state_name(state).into(), value),
                    None => colors.remove(state_name(state)),
                };
                if !colors.is_empty() {
                    config.insert("colors".into(), Value::Object(colors));
                }
            }
            Edit::ResetColors => {
                config.remove("colors");
            }
            Edit::Feature(feature, on) => {
                config.insert(feature.key().into(), json!(on));
            }
        }
    }

    /// The status line after the change was saved.
    pub fn describe(self) -> String {
        match self {
            Edit::Theme(theme) => format!("Theme: {}.", theme.name()),
            Edit::Color(state, Choice::Theme) => format!("{}: theme color.", state_label(state)),
            Edit::Color(state, Choice::Profile) => {
                format!("{}: profile color.", state_label(state))
            }
            Edit::Color(state, Choice::Custom(rgb)) => {
                format!("{}: {}.", state_label(state), hex(rgb))
            }
            Edit::ResetColors => "Every state uses the theme color.".into(),
            Edit::Feature(feature, on) => {
                format!("{}: {}.", feature.label(), if on { "on" } else { "off" })
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Redraw,
    Close,
    Save(Edit),
}

pub fn state_label(state: ProgramState) -> &'static str {
    match state {
        ProgramState::Idle => "Idle",
        ProgramState::Working => "Working",
        ProgramState::Blocked => "Blocked",
        ProgramState::Done => "Done",
        ProgramState::Error => "Error",
    }
}

/// The state of the `/iterm2` panel.
pub struct View {
    pub tab: Tab,
    pub selected: usize,
    pub screen: Screen,
    pub status: Option<(Tone, String)>,
    /// The effective config as an object, for the feature switches.
    pub value: Map<String, Value>,
    pub config: Config,
    /// False outside iTerm2: changes apply to the next iTerm2 session.
    pub iterm2: bool,
    /// The global config file the panel writes.
    pub file: String,
    /// Keys of the project config file, which win over the global file.
    pub project_keys: Vec<String>,
}

impl View {
    pub fn new(iterm2: bool, file: String) -> Self {
        Self {
            tab: Tab::Colors,
            selected: 0,
            screen: Screen::List,
            status: None,
            value: Map::new(),
            config: Config::default(),
            iterm2,
            file,
            project_keys: Vec::new(),
        }
    }

    /// Replaces the effective config after a load or a save.
    pub fn set_value(&mut self, value: Map<String, Value>) {
        self.config = Config::from_value(&Value::Object(value.clone()));
        self.value = value;
    }

    pub fn row(&self) -> Row {
        let rows = self.tab.rows();
        rows[self.selected.min(rows.len() - 1)]
    }

    /// The color the tab shows while the panel is open; `None` shows the agent state.
    pub fn preview(&self) -> Option<Option<[u8; 3]>> {
        let palette = self.config.palette;
        match (self.row(), &self.screen) {
            (Row::Theme, Screen::Menu { choice }) => {
                let theme = Theme::ALL[(*choice).min(Theme::ALL.len() - 1)];
                Some(theme.color(ProgramState::Working))
            }
            (Row::Theme, _) => Some(palette.color(ProgramState::Working)),
            (Row::State(state), Screen::Menu { choice: 0 }) => Some(palette.theme.color(state)),
            (Row::State(_), Screen::Menu { choice: 1 }) => Some(None),
            (Row::State(state), Screen::Hex { text, .. }) => {
                Some(parse_hex(text).or_else(|| palette.color(state)))
            }
            (Row::State(state), _) => Some(palette.color(state)),
            (Row::Reset | Row::Feature(_), _) => None,
        }
    }

    /// The text `/iterm2` prints without a UI.
    pub fn summary(&self) -> String {
        let palette = self.config.palette;
        let mut lines = vec![format!("iTerm2 tab colors ({})", self.file)];
        lines.push(format!("theme: {}", palette.theme.name()));
        for state in COLORED {
            let source = match palette.choice(state) {
                Choice::Theme => "theme",
                Choice::Profile => "profile",
                Choice::Custom(_) => "custom",
            };
            let color = palette.color(state).map_or("profile".into(), hex);
            lines.push(format!("{}: {color} ({source})", state_name(state)));
        }
        for feature in Feature::ALL {
            let on = if feature.on(&self.value) { "on" } else { "off" };
            lines.push(format!("{}: {on}", feature.key()));
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests;
