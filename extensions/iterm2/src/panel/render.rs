//! Lines of the `/iterm2` panel: rule, title, tabs, notices, body, status, hints, rule.

use pi_ext_sdk::style::{self, Color};

use super::{Feature, Row, Screen, Tab, Tone, View, state_label};
use crate::sequences::{ProgramState, state_name};
use crate::setup::wrap;
use crate::themes::{COLORED, Choice, Theme, black_title, hex, parse_hex};

/// Columns line up and the descriptions show from this width.
const WIDE: usize = 72;

pub fn accent(text: &str) -> String {
    style::fg(Color::Cyan, text)
}

pub fn muted(text: &str) -> String {
    style::fg(Color::Gray, text)
}

pub fn warning(text: &str) -> String {
    style::fg(Color::Yellow, text)
}

/// `text` on `color` in the title ink iTerm2 picks; muted text for the profile color.
fn swatch(color: Option<[u8; 3]>, text: &str) -> String {
    match color {
        Some(rgb @ [r, g, b]) => {
            let ink = if black_title(rgb) {
                Color::Rgb(0, 0, 0)
            } else {
                Color::Rgb(255, 255, 255)
            };
            style::bg(Color::Rgb(r, g, b), &style::fg(ink, text))
        }
        None => muted(text),
    }
}

fn pointer(selected: bool) -> String {
    if selected {
        accent("→ ")
    } else {
        "  ".into()
    }
}

impl View {
    /// `height` is the lines the panel may use; blank lines go first when it is short.
    pub fn render(&self, width: usize, height: Option<usize>) -> Vec<String> {
        let width = width.max(20);
        let rule = muted(&"─".repeat(width));
        let mut lines = vec![rule.clone(), self.title(), self.tabs_line()];
        lines.extend(self.notices(width));
        lines.push(String::new());
        lines.extend(match &self.screen {
            Screen::List => self.list(width),
            Screen::Menu { choice } => self.menu(*choice, width),
            Screen::Hex { state, text } => self.hex_field(*state, text),
        });
        lines.push(String::new());
        if let Some((tone, text)) = &self.status {
            for line in wrap(text, width.saturating_sub(2)) {
                let line = format!(" {line}");
                lines.push(match tone {
                    Tone::Info => accent(&line),
                    Tone::Warn => warning(&line),
                });
            }
        }
        lines.extend(hint_lines(&self.hints(), width));
        lines.push(rule);
        if let Some(height) = height.filter(|h| lines.len() > *h) {
            lines.retain(|line| !line.is_empty());
            lines.truncate(height.max(1));
        }
        lines
            .into_iter()
            .map(|line| style::truncate(&line, width))
            .collect()
    }

    fn title(&self) -> String {
        let theme = self.config.palette.theme.name();
        let summary = format!("  theme: {theme} · {}", self.file);
        format!(" {}{}", accent(&style::bold("iTerm2")), muted(&summary))
    }

    fn tabs_line(&self) -> String {
        let tabs: Vec<String> = Tab::ALL
            .iter()
            .map(|tab| {
                if *tab == self.tab {
                    accent(&style::bold(&format!("[{}]", tab.title())))
                } else {
                    muted(tab.title())
                }
            })
            .collect();
        format!(" {}", tabs.join("  "))
    }

    fn notices(&self, width: usize) -> Vec<String> {
        let mut notices = Vec::new();
        if !self.iterm2 {
            notices.push("This terminal is not iTerm2. Changes apply in iTerm2 sessions.".into());
        }
        if !self.project_keys.is_empty() {
            notices.push(format!(
                "The project file .pi/ext/iterm2.json sets {}, which wins over these changes.",
                self.project_keys.join(", ")
            ));
        }
        notices
            .iter()
            .flat_map(|text| wrap(text, width.saturating_sub(2)))
            .map(|line| warning(&format!(" {line}")))
            .collect()
    }

    fn list(&self, width: usize) -> Vec<String> {
        let selected = self.row();
        self.tab
            .rows()
            .into_iter()
            .map(|row| self.row_line(row, row == selected, width))
            .collect()
    }

    fn row_line(&self, row: Row, selected: bool, width: usize) -> String {
        let label_width = match self.tab {
            Tab::Colors => 9,
            Tab::Features => 22,
        };
        let label = |text: &str| {
            let text = style::fit(text, label_width);
            if selected { accent(&text) } else { text }
        };
        let about = |text: &str| {
            if width >= WIDE {
                muted(&format!("  {text}"))
            } else {
                String::new()
            }
        };
        let pointer = pointer(selected);
        match row {
            Row::Theme => {
                let theme = self.config.palette.theme;
                format!(
                    " {pointer}{}{}  {}{}",
                    label("Theme"),
                    accent(&style::fit(theme.name(), 6)),
                    theme_swatches(theme),
                    about(theme.about())
                )
            }
            Row::State(state) => {
                let palette = self.config.palette;
                let color = palette.color(state);
                let source = match palette.choice(state) {
                    Choice::Theme => "theme",
                    Choice::Profile => "profile",
                    Choice::Custom(_) => "custom",
                };
                let value = color.map_or("profile".into(), hex);
                format!(
                    " {pointer}{}{} {} {}",
                    label(state_label(state)),
                    swatch(color, &style::fit(&format!(" {} ", state_name(state)), 10)),
                    style::fit(&value, 8),
                    muted(source)
                )
            }
            Row::Reset => {
                let text = "Use the theme color for every state";
                if selected {
                    format!(" {pointer}{}", accent(text))
                } else {
                    format!(" {pointer}{}", muted(text))
                }
            }
            Row::Feature(feature) => {
                let on = feature.on(&self.value);
                let value = if on { "on" } else { "off" };
                let mut line = format!(
                    " {pointer}{}{}{}",
                    label(feature.label()),
                    accent(&style::fit(value, 4)),
                    about(feature.about())
                );
                if feature != Feature::Enabled && on && !Feature::Enabled.on(&self.value) {
                    line.push_str(&muted("  (integration off)"));
                }
                line
            }
        }
    }

    /// The selected row and its values.
    fn menu(&self, choice: usize, width: usize) -> Vec<String> {
        let row = self.row();
        let mut lines = vec![self.row_line(row, true, width), String::new()];
        let palette = self.config.palette;
        let items: Vec<(String, bool)> = match row {
            Row::Theme => Theme::ALL
                .iter()
                .map(|theme| {
                    let text = format!(
                        "{} {}  {}",
                        style::fit(theme.name(), 6),
                        theme_swatches(*theme),
                        theme.about()
                    );
                    (text, *theme == palette.theme)
                })
                .collect(),
            Row::State(state) => {
                let current = palette.choice(state);
                let theme_color = palette.theme.color(state).map_or("profile".into(), hex);
                vec![
                    (
                        format!("Theme color ({theme_color})"),
                        current == Choice::Theme,
                    ),
                    ("Profile color".into(), current == Choice::Profile),
                    ("Custom color…".into(), matches!(current, Choice::Custom(_))),
                ]
            }
            Row::Feature(feature) => {
                let on = feature.on(&self.value);
                vec![("on".into(), on), ("off".into(), !on)]
            }
            Row::Reset => Vec::new(),
        };
        for (index, (text, current)) in items.into_iter().enumerate() {
            let chosen = index == choice;
            let text = if chosen { accent(&text) } else { text };
            let mark = if current {
                muted("  (current)")
            } else {
                String::new()
            };
            lines.push(format!("   {}{text}{mark}", pointer(chosen)));
        }
        lines
    }

    fn hex_field(&self, state: ProgramState, text: &str) -> Vec<String> {
        let preview = match parse_hex(text) {
            Some(rgb) => swatch(Some(rgb), &format!(" π {} ", state_name(state))),
            None => muted("Type six hex digits, for example #1b506a."),
        };
        vec![
            format!(" {}", style::bold(&format!("{} color", state_label(state)))),
            format!(" › {text}▏"),
            format!(" {preview}"),
        ]
    }
}

/// Joins hints with " · " and starts a new line before one would overflow `width`.
fn hint_lines(hints: &[&str], width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for hint in hints {
        let next = if line.is_empty() {
            format!(" {hint}")
        } else {
            format!("{line} · {hint}")
        };
        if style::visible_width(&next) > width && !line.is_empty() {
            lines.push(muted(&line));
            line = format!(" {hint}");
        } else {
            line = next;
        }
    }
    lines.push(muted(&line));
    lines
}

/// One small block per state in the theme's colors; `··` for the profile color.
fn theme_swatches(theme: Theme) -> String {
    COLORED
        .map(|state| match theme.color(state) {
            Some(rgb) => swatch(Some(rgb), "  "),
            None => muted("··"),
        })
        .join(" ")
}
