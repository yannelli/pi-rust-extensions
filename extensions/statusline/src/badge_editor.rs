//! The `Badge colors` editor of `/statusline`: text and background colors for one badge at a time.

use pi_ext_sdk::Key;
use pi_ext_sdk::style::{self, truncate};
use serde_json::Value;

use crate::badge_colors::{
    BACKGROUND_HELP, CUSTOM, INHERIT, KEYS, TEXT_HELP, custom_hex, label, menu_items, set_color,
};
use crate::colors::{RESET, ThemeColor, foreground};
use crate::editor_items::choice;
use crate::items::{Item, ItemKind};
use crate::list::{ListOutput, SettingItem, SettingsList};
use crate::themes::parse_hex;

#[derive(Clone, Debug, PartialEq)]
pub enum BadgeOutput {
    None,
    Apply(Vec<Vec<Value>>),
    Cancel,
}

struct Custom {
    badge: usize,
    key: &'static str,
    input: String,
    error: Option<String>,
}

pub struct BadgeEditor {
    lines: Vec<Vec<Value>>,
    badges: Vec<(usize, usize)>,
    list: SettingsList,
    menu: Option<(usize, SettingsList)>,
    custom: Option<Custom>,
}

impl BadgeEditor {
    /// Badges are the non-fill entries of `lines`, by position, so duplicates stay separate.
    pub fn new(lines: Vec<Vec<Value>>) -> Self {
        let badges = lines
            .iter()
            .enumerate()
            .flat_map(|(row, line)| {
                line.iter().enumerate().filter_map(move |(index, value)| {
                    Item::parse(value)
                        .filter(|item| !item.is_fill())
                        .map(|_| (row, index))
                })
            })
            .collect();
        let mut editor = Self {
            lines,
            badges,
            list: SettingsList::default(),
            menu: None,
            custom: None,
        };
        editor.list = SettingsList::new(editor.badge_items());
        editor
    }

    pub fn lines(&self) -> &[Vec<Value>] {
        &self.lines
    }

    fn entry(&self, badge: usize) -> &Value {
        let (row, index) = self.badges[badge];
        &self.lines[row][index]
    }

    fn color(&self, badge: usize, key: &str) -> String {
        self.entry(badge)[key]
            .as_str()
            .unwrap_or(INHERIT)
            .to_string()
    }

    fn name(&self, badge: usize) -> String {
        let (row, index) = self.badges[badge];
        let name = match Item::parse(self.entry(badge)).map(|item| item.kind) {
            Some(ItemKind::Segment(segment)) => segment.name().to_string(),
            Some(ItemKind::Text(text)) => format!("text \"{text}\""),
            None => "?".into(),
        };
        format!("{}.{} {name}", row + 1, index + 1)
    }

    fn badge_items(&self) -> Vec<SettingItem> {
        let mut items: Vec<SettingItem> = (0..self.badges.len())
            .map(|badge| {
                let mut current = format!(
                    "text {} · background {}",
                    self.color(badge, "foreground"),
                    self.color(badge, "background")
                );
                if let Some(legacy) = self.entry(badge)["color"].as_str() {
                    current.push_str(&format!(" · legacy color {legacy}"));
                }
                let description =
                    "Colors this badge only; the same module elsewhere keeps its own colors.";
                choice(
                    &format!("badge:{badge}"),
                    &self.name(badge),
                    description,
                    &current,
                    &[],
                )
            })
            .collect();
        if items.is_empty() {
            items.push(choice(
                "none",
                "No badges",
                "Add modules in Modules / order first.",
                "",
                &[],
            ));
        }
        items.push(choice(
            "apply",
            "Apply colors",
            "Saves badge colors. Esc discards unsaved changes.",
            "",
            &[],
        ));
        items
    }

    fn menu_items(&self, badge: usize) -> Vec<SettingItem> {
        menu_items(KEYS.map(|key| self.color(badge, key)))
    }

    fn open_menu(&mut self, badge: usize, select: &str) {
        let mut menu = SettingsList::new(self.menu_items(badge));
        menu.select_item(select);
        self.menu = Some((badge, menu));
    }

    pub fn handle_key(&mut self, key: &Key) -> BadgeOutput {
        if let Some(mut custom) = self.custom.take() {
            match key {
                Key::Escape | Key::Ctrl('c') => return BadgeOutput::None,
                Key::Enter => match custom_hex(&custom.input) {
                    Some(hex) => {
                        set_color(
                            &mut self.lines,
                            self.badges[custom.badge],
                            custom.key,
                            Some(&hex),
                        );
                        self.open_menu(custom.badge, custom.key);
                        return BadgeOutput::None;
                    }
                    None => {
                        custom.error = Some("Enter a color as #rrggbb, for example #56b6c2.".into())
                    }
                },
                Key::Backspace => {
                    custom.input.pop();
                    custom.error = None;
                }
                Key::Ctrl('u') => {
                    custom.input.clear();
                    custom.error = None;
                }
                Key::Char(c) => {
                    custom.input.push(*c);
                    custom.error = None;
                }
                Key::Text(text) => {
                    custom.input.push_str(text);
                    custom.error = None;
                }
                _ => {}
            }
            self.custom = Some(custom);
            return BadgeOutput::None;
        }
        if let Some((badge, mut menu)) = self.menu.take() {
            match menu.handle_key(key) {
                ListOutput::Cancel => {
                    self.list
                        .replace(self.badge_items(), &format!("badge:{badge}"));
                    return BadgeOutput::None;
                }
                ListOutput::Change(id, _) if id == "reset" => {
                    for key in KEYS {
                        set_color(&mut self.lines, self.badges[badge], key, None);
                    }
                    menu.replace(self.menu_items(badge), &id);
                }
                ListOutput::Change(id, value) => {
                    let key = if id == "background" { KEYS[1] } else { KEYS[0] };
                    if value == CUSTOM {
                        let current = self.color(badge, key);
                        let input = custom_hex(&current).unwrap_or_default();
                        self.custom = Some(Custom {
                            badge,
                            key,
                            input,
                            error: None,
                        });
                    } else {
                        let color = (value != INHERIT).then_some(value.as_str());
                        set_color(&mut self.lines, self.badges[badge], key, color);
                    }
                    menu.replace(self.menu_items(badge), &id);
                }
                ListOutput::None | ListOutput::Open(_) => {}
            }
            self.menu = Some((badge, menu));
            return BadgeOutput::None;
        }
        match self.list.handle_key(key) {
            ListOutput::Cancel => BadgeOutput::Cancel,
            ListOutput::Change(id, _) if id == "apply" => BadgeOutput::Apply(self.lines.clone()),
            ListOutput::Change(id, _) => {
                if let Some(badge) = id.strip_prefix("badge:").and_then(|n| n.parse().ok()) {
                    self.open_menu(badge, KEYS[0]);
                }
                BadgeOutput::None
            }
            ListOutput::None | ListOutput::Open(_) => BadgeOutput::None,
        }
    }

    /// The draft with the highlighted swatch or a valid custom color applied; Esc drops either.
    pub fn preview_lines(&self) -> Vec<Vec<Value>> {
        let mut lines = self.lines.clone();
        if let Some(custom) = &self.custom {
            if let Some(hex) = custom_hex(&custom.input) {
                set_color(
                    &mut lines,
                    self.badges[custom.badge],
                    custom.key,
                    Some(&hex),
                );
            }
        } else if let Some((badge, menu)) = &self.menu {
            let highlighted = menu
                .highlighted()
                .filter(|(id, value)| KEYS.contains(&id.as_str()) && value != CUSTOM);
            if let Some((id, value)) = highlighted {
                let color = (value != INHERIT).then_some(value.as_str());
                set_color(&mut lines, self.badges[*badge], &id, color);
            }
        }
        lines
    }

    pub fn render(&self, width: usize) -> Vec<String> {
        let mut lines = vec![style::bold("Badge colors")];
        if let Some(custom) = &self.custom {
            lines.push(format!(
                "{} · {}",
                label(custom.key),
                self.name(custom.badge)
            ));
            let sample = parse_hex(&custom_hex(&custom.input).unwrap_or_default())
                .map(|rgb| format!("  {}■■■{RESET}", foreground(rgb)))
                .unwrap_or_default();
            lines.push(format!("Hex: {}{sample}", custom.input));
            match &custom.error {
                Some(error) => lines.push(ThemeColor::Error.paint(error)),
                None => lines.push(style::dim(if custom.key == "background" {
                    BACKGROUND_HELP
                } else {
                    TEXT_HELP
                })),
            }
            lines.push(style::dim(
                "Type #rrggbb, for example #56b6c2 · Ctrl+U clear · Enter apply · Esc back",
            ));
        } else if let Some((badge, menu)) = &self.menu {
            lines.push(self.name(*badge));
            lines.extend(menu.render(width));
        } else {
            lines.push(style::dim(
                "Pick a badge. Background shows only in filled shapes.",
            ));
            lines.extend(self.list.render(width));
        }
        lines
            .into_iter()
            .map(|line| truncate(&line, width))
            .collect()
    }
}
