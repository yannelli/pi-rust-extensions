//! The `builtin` status line, drawn natively without a process. A preset gives the defaults;
//! `chip`, `theme`, `icons`, `segments`, `separator`, and `colors` override them.

use std::collections::BTreeMap;

use pi_ext_sdk::style::{truncate, visible_width};
use serde_json::Value;

use crate::chips::{Chip, Fg, Piece, join};
use crate::colors::ThemeColor;
use crate::config::StatusLineSettings;
use crate::icons::Icons;
use crate::model::StatusSnapshot;
use crate::presets::{PRESETS, Preset};
use crate::segments::{Item, ItemKind, Segment, segment_text};
use crate::themes::{Palette, Role, parse_badge_hex, parse_hex};

const MIN_RIGHT: usize = 8;

#[derive(Clone, Debug, PartialEq)]
pub struct BuiltinStatusLine {
    pub chip: Chip,
    /// `None` uses pi's theme tokens for text and the `pi-rust` colors for blocks.
    pub palette: Option<Palette>,
    pub icons: Icons,
    pub lines: Vec<Vec<Item>>,
    pub separator: String,
    pub colors: BTreeMap<String, String>,
    pub padding: usize,
    pub layout: String,
}

impl BuiltinStatusLine {
    pub fn from_settings(settings: &StatusLineSettings, padding: usize) -> Self {
        let name = settings.preset.as_deref().unwrap_or("minimal");
        let preset = PRESETS
            .iter()
            .find(|preset| preset.name == name)
            .unwrap_or_else(|| {
                PRESETS
                    .iter()
                    .find(|preset| preset.name == "minimal")
                    .expect("minimal preset")
            });
        let chip = settings
            .chip
            .as_deref()
            .or(settings.style.as_deref())
            .and_then(Chip::from_name);
        let theme = settings.theme.as_deref().unwrap_or(preset.theme);
        Self {
            chip: chip.unwrap_or(preset.chip),
            palette: Palette::named(theme),
            icons: settings
                .icons
                .as_deref()
                .and_then(Icons::from_name)
                .unwrap_or(preset.icons),
            lines: settings
                .segments
                .as_ref()
                .and_then(parse_lines)
                .unwrap_or_else(|| preset_lines(preset)),
            separator: settings.separator.clone().unwrap_or_else(|| match chip {
                Some(chip) if chip != preset.chip => chip.default_separator().to_string(),
                _ => preset.separator.to_string(),
            }),
            colors: settings.colors.clone().unwrap_or_default(),
            padding,
            layout: settings.layout.clone().unwrap_or_else(|| "spread".into()),
        }
    }

    /// A Codex `[tui] status_line` list, drawn as Codex draws it: plain text joined by ` · `.
    pub fn from_codex(items: &[String], padding: usize) -> Self {
        let codex = PRESETS
            .iter()
            .find(|preset| preset.name == "codex")
            .expect("codex preset");
        let line: Vec<Item> = items
            .iter()
            .filter_map(|name| Segment::from_name(name))
            .map(Item::segment)
            .collect();
        Self {
            chip: codex.chip,
            palette: Palette::named(codex.theme),
            icons: codex.icons,
            lines: if line.is_empty() {
                preset_lines(codex)
            } else {
                vec![line]
            },
            separator: codex.separator.to_string(),
            colors: BTreeMap::new(),
            padding,
            layout: "compact".into(),
        }
    }

    /// The status lines for `width` columns; empty when no segment has anything to show.
    pub fn render(&self, snapshot: &StatusSnapshot, width: usize) -> Vec<String> {
        if width <= self.padding {
            return Vec::new();
        }
        let inner = width.saturating_sub(self.padding);
        let pad = " ".repeat(width.min(self.padding));
        let lines = self.lines.iter().flat_map(|line| {
            let rows: Vec<&[Item]> = if self.layout == "stacked" {
                line.split(Item::is_fill).collect()
            } else {
                vec![line.as_slice()]
            };
            rows.into_iter()
                .filter_map(|row| self.render_line(row, snapshot, inner))
        });
        lines.map(|line| format!("{pad}{line}")).collect()
    }

    fn render_line(
        &self,
        line: &[Item],
        snapshot: &StatusSnapshot,
        width: usize,
    ) -> Option<String> {
        let split = if self.layout == "compact" {
            line.len()
        } else {
            line.iter().position(Item::is_fill).unwrap_or(line.len())
        };
        let pieces = |items: &[Item]| -> Vec<Piece> {
            items
                .iter()
                .filter_map(|item| self.piece(item, snapshot))
                .collect()
        };
        let (left, right) = (
            pieces(&line[..split]),
            pieces(line.get(split + 1..).unwrap_or_default()),
        );
        if left.is_empty() && right.is_empty() {
            return None;
        }
        let left = join(self.chip, &left, &self.separator, false);
        let right = join(self.chip, &right, &self.separator, true);
        let (left_width, right_width) = (visible_width(&left), visible_width(&right));
        if left_width == 0 || right_width == 0 {
            let line = if left_width > 0 { left } else { right };
            return Some(truncate(&line, width));
        }
        if left_width + 1 + right_width <= width {
            return Some(format!(
                "{left}{}{right}",
                " ".repeat(width - left_width - right_width)
            ));
        }
        // A right side that does not fit is cut from its end, while at least 8 columns remain for it.
        if left_width + 1 + MIN_RIGHT <= width {
            let right = truncate(&right, width - left_width - 1);
            let gap = width - left_width - visible_width(&right);
            return Some(format!("{left}{}{right}", " ".repeat(gap)));
        }
        Some(truncate(&left, width))
    }

    fn piece(&self, item: &Item, snapshot: &StatusSnapshot) -> Option<Piece> {
        let (text, role, name, icon) = match &item.kind {
            ItemKind::Segment(segment) => (
                segment_text(*segment, snapshot)?,
                segment.role(snapshot),
                Some(segment.name()),
                self.icons.icon(*segment),
            ),
            ItemKind::Text(text) => (text.clone(), Role::Meta, None, None),
        };
        if text.trim().is_empty() {
            return None;
        }
        let icon = item
            .icon
            .as_deref()
            .or(icon)
            .filter(|icon| !icon.is_empty());
        let text = icon.map_or(text.clone(), |icon| format!("{icon} {text}"));
        let custom = item.color.as_deref().or_else(|| {
            name.and_then(|name| self.colors.get(name))
                .map(String::as_str)
        });
        let segment = match item.kind {
            ItemKind::Segment(segment) => Some(segment),
            ItemKind::Text(_) => None,
        };
        let fallback = Palette::named("pi-rust").expect("pi-rust theme");
        let palette = self.palette.as_ref().unwrap_or(&fallback);
        let bg = custom
            .and_then(spec_rgb)
            .unwrap_or_else(|| palette.rgb(role));
        let fg = match (custom, &self.palette) {
            (Some(spec), _) => spec_fg(spec).unwrap_or(Fg::Rgb(bg)),
            (None, Some(palette)) => Fg::Rgb(palette.rgb(role)),
            (None, None) => Fg::Token(token_for(role, segment)),
        };
        // Badge colors that do not parse are ignored, so the legacy colors above apply.
        let bg = item.background.as_deref().and_then(badge_rgb).unwrap_or(bg);
        let ink = item.foreground.as_deref().and_then(badge_fg);
        Some(Piece { text, fg, bg, ink })
    }
}

fn preset_lines(preset: &Preset) -> Vec<Vec<Item>> {
    preset
        .lines
        .iter()
        .map(|line| line.iter().copied().map(Item::segment).collect())
        .collect()
}

/// `["a", "b"]` is one line; `[["a"], ["b"]]` is two. Unknown entries are skipped.
fn parse_lines(value: &Value) -> Option<Vec<Vec<Item>>> {
    let items = |values: &[Value]| values.iter().filter_map(Item::parse).collect::<Vec<_>>();
    let values = value.as_array()?;
    let lines: Vec<Vec<Item>> = if values.iter().any(Value::is_array) {
        values
            .iter()
            .filter_map(Value::as_array)
            .map(|line| items(line))
            .collect()
    } else {
        vec![items(values)]
    };
    let lines: Vec<Vec<Item>> = lines.into_iter().filter(|line| !line.is_empty()).collect();
    Some(lines)
}

/// `#rrggbb` or a pi theme token such as `accent`, as sRGB.
fn spec_rgb(spec: &str) -> Option<[u8; 3]> {
    if spec.starts_with('#') {
        return parse_hex(spec);
    }
    ThemeColor::from_name(spec).map(ThemeColor::rgb)
}

fn spec_fg(spec: &str) -> Option<Fg> {
    match ThemeColor::from_name(spec) {
        Some(token) => Some(Fg::Token(token)),
        None => parse_hex(spec).map(Fg::Rgb),
    }
}

/// A badge `background`: a pi theme token, `#rrggbb`, or `#rgb`, as sRGB.
fn badge_rgb(spec: &str) -> Option<[u8; 3]> {
    badge_fg(spec).map(Fg::rgb)
}

/// A badge `foreground`: a pi theme token, `#rrggbb`, or `#rgb`.
fn badge_fg(spec: &str) -> Option<Fg> {
    match ThemeColor::from_name(spec) {
        Some(token) => Some(Fg::Token(token)),
        None => parse_badge_hex(spec).map(Fg::Rgb),
    }
}

/// The pi theme token for text without a theme, as the default footer colors it.
fn token_for(role: Role, segment: Option<Segment>) -> ThemeColor {
    match role {
        Role::ContextHigh => ThemeColor::Error,
        Role::ContextWarn => ThemeColor::Warning,
        Role::ContextOk if segment == Some(Segment::ContextBar) => ThemeColor::Success,
        Role::Model | Role::State => ThemeColor::Accent,
        _ => ThemeColor::Dim,
    }
}
