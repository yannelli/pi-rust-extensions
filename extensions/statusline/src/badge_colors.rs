//! The color choices of the `Badge colors` editor and the edits it makes to `segments` rows.

use serde_json::{Value, json};

use crate::builtin::BuiltinStatusLine;
use crate::config::StatusLineSettings;
use crate::editor_items::choice;
use crate::layout::layout_values;
use crate::list::SettingItem;

pub const INHERIT: &str = "inherit";
pub const CUSTOM: &str = "custom #rrggbb";
pub const TOKENS: [&str; 8] = [
    "accent", "text", "muted", "dim", "success", "warning", "error", "border",
];
pub const SWATCHES: [&str; 10] = [
    "#e06c75", "#e5c07b", "#98c379", "#56b6c2", "#61afef", "#c678dd", "#d19a66", "#282c34",
    "#ffffff", "#000000",
];
pub const KEYS: [&str; 2] = ["foreground", "background"];
pub const TEXT_HELP: &str = "Text color of this badge. Plain, brackets and dot shapes show only \
    this color; filled shapes draw the text on the background.";
pub const BACKGROUND_HELP: &str = "Fill of this badge. Only filled shapes (block, rounded, \
    powerline, slanted) show it; plain, brackets and dot ignore it.";

/// `#rrggbb` in lowercase, or `None` for any other text.
pub fn custom_hex(input: &str) -> Option<String> {
    let hex = input.trim().strip_prefix('#')?;
    (hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex.to_lowercase()))
}

/// The rows of `effective` as saved, so unknown keys stay; the preset rows when it has no `segments`.
pub fn badge_lines(effective: &Value) -> Vec<Vec<Value>> {
    match effective.get("segments").and_then(Value::as_array) {
        Some(values) if values.iter().any(Value::is_array) => {
            values.iter().filter_map(Value::as_array).cloned().collect()
        }
        Some(values) => vec![values.clone()],
        None => {
            let settings: StatusLineSettings =
                serde_json::from_value(effective.clone()).unwrap_or_default();
            layout_values(&BuiltinStatusLine::from_settings(&settings, 0))
        }
    }
}

pub fn label(key: &str) -> &'static str {
    if key == "background" {
        "Background"
    } else {
        "Text color"
    }
}

pub fn menu_items(colors: [String; 2]) -> Vec<SettingItem> {
    let mut items: Vec<SettingItem> = KEYS
        .iter()
        .zip([TEXT_HELP, BACKGROUND_HELP])
        .zip(colors)
        .map(|((key, help), current)| {
            let mut values: Vec<&str> = std::iter::once(INHERIT)
                .chain(TOKENS)
                .chain(SWATCHES)
                .collect();
            if !values.contains(&current.as_str()) {
                values.push(&current);
            }
            values.push(CUSTOM);
            choice(key, label(key), help, &current, &values)
        })
        .collect();
    items.push(choice(
        "reset",
        "Reset badge colors",
        "Inherit the palette again. Legacy color and icon stay.",
        "",
        &[],
    ));
    items
}

pub fn set_color(
    lines: &mut [Vec<Value>],
    (row, index): (usize, usize),
    key: &str,
    color: Option<&str>,
) {
    let entry = &mut lines[row][index];
    if let Some(name) = entry.as_str() {
        *entry = json!({"segment": name});
    }
    let Some(object) = entry.as_object_mut() else {
        return;
    };
    match color {
        Some(color) => object.insert(key.into(), json!(color)),
        None => object.remove(key),
    };
}
