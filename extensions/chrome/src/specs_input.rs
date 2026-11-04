use pi_ext_sdk::ToolSpec;
use serde_json::json;

use crate::snapshot::include_snapshot;
use crate::specs::{b, n, point_props, s, schema, tool, with_target};

pub fn input_specs() -> Vec<ToolSpec> {
    let mut click = point_props();
    click.push(("domFallback", json!({ "type": "boolean", "default": true })));
    click.extend(include_snapshot());
    let mut typing = vec![
        ("text", json!({ "type": "string" })),
        ("uid", json!({ "type": "string" })),
        ("selector", json!({ "type": "string" })),
        ("replace", json!({ "type": "boolean" })),
        ("perCharacter", json!({ "type": "boolean" })),
        ("pressEnter", json!({ "type": "boolean" })),
    ];
    typing.extend(include_snapshot());
    let mut fill = vec![
        ("text", json!({ "type": "string" })),
        ("uid", json!({ "type": "string" })),
        ("selector", json!({ "type": "string" })),
        ("perCharacter", json!({ "type": "boolean" })),
        ("submit", b("Press Enter after.")),
    ];
    fill.extend(include_snapshot());
    let mut key = vec![
        ("key", s("Enter, ArrowDown, a character, etc.")),
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
        ("deltaY", n("Pixels; positive is down")),
        ("deltaX", n("Pixels; positive is right")),
        ("steps", json!({ "type": "number" })),
    ]);
    vec![
        tool(
            "chrome_click",
            "Chrome Click",
            "Click a uid, selector, or point with mouse input.",
            "Click Chrome page elements.",
            schema(with_target(click), &[]),
        ),
        tool(
            "chrome_type",
            "Chrome Type",
            "Type text with key input at the end of the field. replace=true replaces the contents.",
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
            "Press one key.",
            "Press keys in Chrome.",
            schema(with_target(key), &["key"]),
        ),
        tool(
            "chrome_hover",
            "Chrome Hover",
            "Move the mouse over a uid, selector, or point.",
            "Hover a Chrome element.",
            schema(with_target(point_props()), &[]),
        ),
        tool(
            "chrome_scroll",
            "Chrome Scroll",
            "Scroll the page, or the element at a uid or selector, with wheel events.",
            "Scroll a Chrome page.",
            schema(with_target(scroll), &[]),
        ),
        tool(
            "chrome_upload_file",
            "Chrome Upload File",
            "Attach local files to an <input type=file>.",
            "Attach files to a Chrome file input.",
            schema(
                with_target(vec![
                    ("uid", json!({ "type": "string" })),
                    ("selector", json!({ "type": "string" })),
                    (
                        "paths",
                        json!({ "type": "array", "items": { "type": "string" }, "description": "Relative to the project folder" }),
                    ),
                ]),
                &["paths"],
            ),
        ),
    ]
}
