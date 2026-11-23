//! Tab color themes and the per-state colors of the config (`theme`, `colors`).

use serde_json::{Map, Value};

use crate::sequences::{ProgramState, state_name};

/// The states that can color the tab, in panel and config order. Idle uses the profile color.
pub const COLORED: [ProgramState; 4] = [
    ProgramState::Working,
    ProgramState::Blocked,
    ProgramState::Done,
    ProgramState::Error,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    Dark,
    Light,
    Vivid,
    Quiet,
}

impl Theme {
    pub const ALL: [Theme; 4] = [Theme::Dark, Theme::Light, Theme::Vivid, Theme::Quiet];

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|theme| theme.name() == name)
    }

    pub fn name(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
            Theme::Vivid => "vivid",
            Theme::Quiet => "quiet",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Theme::Dark => "Dark tints with white titles, for dark profiles",
            Theme::Light => "Light pastels with black titles, for light profiles",
            Theme::Vivid => "Saturated colors with white titles",
            Theme::Quiet => "Dark tints only while the agent works or waits for you",
        }
    }

    /// The colors in [`COLORED`] order; `None` keeps the profile color.
    // iTerm2 picks tab-title ink at a brightness cutoff, not WCAG contrast. Each theme keeps one
    // ink on every state, including the inactive-tab blend toward middle gray.
    fn colors(self) -> [Option<[u8; 3]>; 4] {
        match self {
            Theme::Dark => [
                Some([27, 80, 106]),
                Some([106, 88, 27]),
                Some([27, 106, 70]),
                Some([106, 27, 36]),
            ],
            Theme::Light => [
                Some([132, 184, 255]),
                Some([242, 202, 114]),
                Some([151, 211, 163]),
                Some([245, 160, 164]),
            ],
            Theme::Vivid => [
                Some([9, 105, 218]),
                Some([138, 90, 0]),
                Some([17, 99, 41]),
                Some([207, 34, 46]),
            ],
            Theme::Quiet => [Some([27, 80, 106]), Some([106, 88, 27]), None, None],
        }
    }

    pub fn color(self, state: ProgramState) -> Option<[u8; 3]> {
        let index = COLORED.iter().position(|s| *s == state)?;
        self.colors()[index]
    }
}

/// What one state shows: the theme color, the profile color, or a custom color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Choice {
    #[default]
    Theme,
    Profile,
    Custom([u8; 3]),
}

impl Choice {
    /// A `colors` value: `"profile"` or `"#rrggbb"`. Other values fall back to the theme.
    fn parse(value: &Value) -> Self {
        match value.as_str() {
            Some("profile") => Choice::Profile,
            Some(text) => parse_hex(text).map_or(Choice::Theme, Choice::Custom),
            None => Choice::Theme,
        }
    }

    /// The `colors` value; `None` for the theme color, which the config leaves out.
    pub fn json(self) -> Option<Value> {
        match self {
            Choice::Theme => None,
            Choice::Profile => Some(Value::from("profile")),
            Choice::Custom(rgb) => Some(Value::from(hex(rgb))),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Palette {
    pub theme: Theme,
    /// In [`COLORED`] order.
    pub choices: [Choice; 4],
}

impl Palette {
    pub fn from_config(config: &Map<String, Value>) -> Self {
        let theme = config
            .get("theme")
            .and_then(Value::as_str)
            .and_then(Theme::parse)
            .unwrap_or_default();
        let colors = config.get("colors").and_then(Value::as_object);
        let choices = COLORED.map(|state| {
            colors
                .and_then(|c| c.get(state_name(state)))
                .map_or(Choice::Theme, Choice::parse)
        });
        Self { theme, choices }
    }

    pub fn choice(&self, state: ProgramState) -> Choice {
        COLORED
            .iter()
            .position(|s| *s == state)
            .map_or(Choice::Profile, |index| self.choices[index])
    }

    /// The tab color for a state; `None` keeps the profile color.
    pub fn color(&self, state: ProgramState) -> Option<[u8; 3]> {
        match self.choice(state) {
            Choice::Theme => self.theme.color(state),
            Choice::Profile => None,
            Choice::Custom(rgb) => Some(rgb),
        }
    }
}

/// `#rrggbb` or `rrggbb`, in any case.
pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let digits = text.trim();
    let digits = digits.strip_prefix('#').unwrap_or(digits);
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

pub fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Whether iTerm2 draws the title of a tab with this color in black (else white).
pub fn black_title(rgb: [u8; 3]) -> bool {
    let [r, g, b] = rgb.map(|c| f64::from(c) / 255.0);
    0.299 * r + 0.587 * g + 0.114 * b > 0.5
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn theme_titles_stay_readable_when_tabs_are_dimmed() {
        let linear = |value: f64| {
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        for theme in Theme::ALL {
            let colors = COLORED.into_iter().filter_map(|s| theme.color(s));
            let inks: Vec<bool> = colors.clone().map(black_title).collect();
            assert!(
                inks.windows(2).all(|w| w[0] == w[1]),
                "{theme:?} mixes inks"
            );
            for rgb in colors {
                let black = black_title(rgb);
                // Selected/unselected tabs in active/inactive windows use these blends in iTerm2.
                for alpha in [1.0, 0.6, 0.4, 0.3] {
                    let [r, g, b] = rgb.map(|c| f64::from(c) / 255.0 * alpha + 0.5 * (1.0 - alpha));
                    let blended = [r, g, b].map(|c| (c * 255.0).round() as u8);
                    assert_eq!(black_title(blended), black, "{theme:?} {rgb:?} at {alpha}");
                    let luminance = 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
                    let contrast = if black {
                        (luminance + 0.05) / 0.05
                    } else {
                        1.05 / (luminance + 0.05)
                    };
                    assert!(contrast >= 4.5, "{theme:?} {rgb:?} at {alpha}: {contrast}");
                }
            }
        }
    }

    #[test]
    fn config_picks_the_theme_and_per_state_colors() {
        let palette = |value: Value| Palette::from_config(value.as_object().unwrap());
        let default = palette(json!({}));
        assert_eq!(default.theme, Theme::Dark);
        assert_eq!(default.color(ProgramState::Working), Some([27, 80, 106]));
        assert_eq!(default.color(ProgramState::Idle), None);

        let custom = palette(json!({
            "theme": "vivid",
            "colors": { "working": "#FF8800", "done": "profile", "error": "red" }
        }));
        assert_eq!(custom.theme, Theme::Vivid);
        assert_eq!(custom.color(ProgramState::Working), Some([255, 136, 0]));
        assert_eq!(custom.color(ProgramState::Done), None);
        assert_eq!(custom.choice(ProgramState::Error), Choice::Theme);
        assert_eq!(custom.color(ProgramState::Error), Some([207, 34, 46]));

        assert_eq!(palette(json!({"theme": "neon"})).theme, Theme::Dark);
        assert_eq!(
            palette(json!({"theme": "quiet"})).color(ProgramState::Done),
            None
        );
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(parse_hex("#1b506a"), Some([27, 80, 106]));
        assert_eq!(parse_hex(" 1B506A "), Some([27, 80, 106]));
        assert_eq!(parse_hex("#1b506"), None);
        assert_eq!(parse_hex("#1b506g"), None);
        assert_eq!(hex([27, 80, 106]), "#1b506a");
        assert_eq!(Choice::Custom([1, 2, 3]).json(), Some(json!("#010203")));
        assert_eq!(Choice::Theme.json(), None);
    }
}
