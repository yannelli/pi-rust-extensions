//! The items of the `/statusline` editor.

use serde_json::Value;

use crate::builtin::BuiltinStatusLine;
use crate::config::StatusLineSettings;
use crate::editor::is_context;
use crate::list::SettingItem;
use crate::presets::preset_names;
use crate::segments::ItemKind;
use crate::themes::theme_names;

pub fn choice(
    id: &str,
    label: &str,
    description: &str,
    current: &str,
    values: &[&str],
) -> SettingItem {
    SettingItem {
        id: id.into(),
        label: label.into(),
        description: description.into(),
        current: current.into(),
        values: values.iter().map(|value| value.to_string()).collect(),
        custom: false,
    }
}

pub fn build_items(value: &Value) -> Vec<SettingItem> {
    let field = |key, fallback| value.get(key).and_then(Value::as_str).unwrap_or(fallback);
    let settings: StatusLineSettings = serde_json::from_value(value.clone()).unwrap_or_default();
    let builtin = BuiltinStatusLine::from_settings(&settings, 0);
    let themes: Vec<&str> = std::iter::once("preset")
        .chain(std::iter::once("terminal"))
        .chain(theme_names())
        .collect();
    let presets: Vec<&str> = preset_names().collect();
    let context: Vec<_> = builtin
        .lines
        .iter()
        .flatten()
        .filter_map(|item| match item.kind {
            ItemKind::Segment(segment) if is_context(segment) => Some(segment.name()),
            _ => None,
        })
        .collect();
    let context = match context.as_slice() {
        [] => "hidden",
        [name] => name,
        _ => "custom",
    };
    let source = match field("type", "builtin") {
        "claude" => "from-claude",
        "cursor" => "from-cursor",
        "codex" => "from-codex",
        source => source,
    };
    vec![
        choice(
            "source",
            "Source",
            "Appearance edits switch to builtin. Existing command settings are kept.",
            source,
            &[
                "builtin",
                "off",
                "command",
                "from-claude",
                "from-cursor",
                "from-codex",
            ],
        ),
        choice(
            "preset",
            "Preset",
            "Resets layout, shape, theme, icons, and separator overrides.",
            field("preset", "minimal"),
            &presets,
        ),
        choice(
            "theme",
            "Palette",
            "Colors for the status line, independent of the harness theme.",
            field("theme", "preset"),
            &themes,
        ),
        choice(
            "chip",
            "Pill shape",
            "Rounded, slanted and powerline shapes need a Nerd Font.",
            field("chip", field("style", "preset")),
            &[
                "preset",
                "plain",
                "block",
                "rounded",
                "powerline",
                "slanted",
                "brackets",
                "dot",
            ],
        ),
        choice(
            "icons",
            "Icons",
            "Choose icons supported by your terminal font.",
            field("icons", "preset"),
            &["preset", "none", "unicode", "nerd", "emoji"],
        ),
        choice(
            "layout",
            "Alignment",
            "Spread honors fill; compact removes flexible gaps; stacked breaks at fill.",
            field("layout", "spread"),
            &["spread", "compact", "stacked"],
        ),
        choice(
            "separator",
            "Separator",
            "Spacing between pills. Block shapes already include inner padding.",
            field("separator", "preset"),
            &["preset", "", " ", "  ", " · ", " | "],
        ),
        choice(
            "padding",
            "Left padding",
            "Spaces before the status line.",
            &settings.padding.unwrap_or(0.0).to_string(),
            &["0", "1", "2", "3"],
        ),
        choice(
            "context",
            "Context display",
            "Replaces context modules. Headroom is context capacity, not subscription quota.",
            context,
            &[
                "context",
                "contextUsed",
                "contextRemaining",
                "contextWindow",
                "contextBar",
                "contextTokens",
                "contextHeadroom",
                "hidden",
            ],
        ),
        SettingItem {
            id: "segments".into(),
            label: "Modules / order".into(),
            current: "edit".into(),
            description: "Add, remove, reorder, or move modules between rows. Apply to save."
                .into(),
            custom: true,
            ..Default::default()
        },
        SettingItem {
            id: "badges".into(),
            label: "Badge colors".into(),
            current: "edit".into(),
            description: "Text and background color of each badge. Background shows only in filled shapes. Apply to save."
                .into(),
            custom: true,
            ..Default::default()
        },
    ]
}
