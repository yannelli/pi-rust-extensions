use pi_ext_sdk::ToolSpec;
use serde_json::json;

use crate::snapshot::include_snapshot;
use crate::specs::{b, n, point_props, s, schema, tool, with_target};

pub fn input_specs() -> Vec<ToolSpec> {
    let mut click = point_props();
    click.push((
        "domFallback",
        b("Default true. Use a DOM click when trusted input fails."),
    ));
    click.extend(include_snapshot());
    let mut typing = vec![
        ("text", json!({ "type": "string" })),
        (
            "uid",
            s("Element uid. The caret goes to the end of the field."),
        ),
        ("selector", s("CSS selector of the field.")),
        ("replace", b("Select and replace the field contents first.")),
        ("perCharacter", b("Send one key event per character.")),
        ("pressEnter", b("Press Enter after typing.")),
    ];
    typing.extend(include_snapshot());
    let mut fill = vec![
        ("text", json!({ "type": "string" })),
        ("uid", s("Element uid.")),
        ("selector", s("CSS selector of the field.")),
        ("perCharacter", b("Send one key event per character.")),
        ("submit", b("Press Enter after filling.")),
    ];
    fill.extend(include_snapshot());
    let mut key = vec![
        (
            "key",
            s(
                "Enter, Escape, Tab, Backspace, Delete, Arrow keys, Home, End, PageUp, PageDown, Space, or one character.",
            ),
        ),
        (
            "modifiers",
            json!({ "type": "object", "properties": {
            "shiftKey": { "type": "boolean" }, "ctrlKey": { "type": "boolean" },
            "altKey": { "type": "boolean" }, "metaKey": { "type": "boolean" } } }),
        ),
    ];
    key.extend(include_snapshot());
    let mut scroll = point_props();
    scroll.extend([
        ("deltaY", n("Pixels down. Negative scrolls up.")),
        ("deltaX", n("Pixels right. Negative scrolls left.")),
        (
            "steps",
            n("Wheel events to send. Default ceil(|deltaY| / 100)."),
        ),
    ]);
    vec![
        tool(
            "chrome_click",
            "Chrome Click",
            "Click an element uid, CSS selector, or viewport point with Chrome mouse input.",
            "Click Chrome page elements.",
            schema(with_target(click), &[]),
        ),
        tool(
            "chrome_type",
            "Chrome Type",
            "Type text with Chrome input. With uid or selector, the text goes at the end of the field. Use chrome_fill or replace=true to replace the contents.",
            "Type text into Chrome.",
            schema(with_target(typing), &["text"]),
        ),
        tool(
            "chrome_fill",
            "Chrome Fill",
            "Replace the value of a text field or contenteditable element.",
            "Fill a Chrome form field.",
            schema(with_target(fill), &["text"]),
        ),
        tool(
            "chrome_key",
            "Chrome Key",
            "Press one key in the tab, with optional modifiers.",
            "Press keys in Chrome.",
            schema(with_target(key), &["key"]),
        ),
        tool(
            "chrome_hover",
            "Chrome Hover",
            "Move the mouse over an element uid, selector, or point.",
            "Hover a Chrome element.",
            schema(with_target(point_props()), &[]),
        ),
        tool(
            "chrome_scroll",
            "Chrome Scroll",
            "Scroll the page or the element under a uid or selector with mouse wheel events.",
            "Scroll a Chrome page.",
            schema(with_target(scroll), &[]),
        ),
        tool(
            "chrome_upload_file",
            "Chrome Upload File",
            "Attach local files to an <input type=file> without the file picker.",
            "Attach files to a Chrome file input.",
            schema(
                with_target(vec![
                    ("uid", s("Element uid of the file input.")),
                    ("selector", s("CSS selector of the file input.")),
                    (
                        "paths",
                        json!({ "type": "array", "items": { "type": "string" }, "description": "File paths. Relative paths start at the project folder." }),
                    ),
                ]),
                &["paths"],
            ),
        ),
    ]
}
