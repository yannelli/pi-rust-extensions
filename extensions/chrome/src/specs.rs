use pi_ext_sdk::ToolSpec;
use serde_json::{Map, Value, json};

use crate::snapshot::{MAX_ELEMENTS, MODES, snapshot_props};

pub const TOOL_NAMES: &[&str] = &[
    "chrome_launch",
    "chrome_tab",
    "chrome_snapshot",
    "chrome_find",
    "chrome_navigate",
    "chrome_evaluate",
    "chrome_wait_for",
    "chrome_screenshot",
    "chrome_cdp",
    "chrome_cdp_targets",
    "chrome_click",
    "chrome_type",
    "chrome_fill",
    "chrome_key",
    "chrome_hover",
    "chrome_scroll",
    "chrome_upload_file",
];

pub(crate) fn s(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

pub(crate) fn n(description: &str) -> Value {
    json!({ "type": "number", "description": description })
}

pub(crate) fn b(description: &str) -> Value {
    json!({ "type": "boolean", "description": description })
}

pub(crate) fn target_props() -> Vec<(&'static str, Value)> {
    vec![
        ("targetId", json!({ "type": "string" })),
        ("urlIncludes", json!({ "type": "string" })),
        ("titleIncludes", json!({ "type": "string" })),
        ("background", json!({ "type": "boolean" })),
    ]
}

pub(crate) fn point_props() -> Vec<(&'static str, Value)> {
    vec![
        ("uid", json!({ "type": "string" })),
        ("selector", json!({ "type": "string" })),
        ("x", json!({ "type": "number" })),
        ("y", json!({ "type": "number" })),
    ]
}

pub(crate) fn schema(props: Vec<(&'static str, Value)>, required: &[&str]) -> Value {
    let mut properties = Map::new();
    for (key, value) in props {
        properties.insert(key.to_owned(), value);
    }
    let mut schema = json!({ "type": "object", "properties": properties });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    schema
}

pub(crate) fn tool(
    name: &str,
    label: &str,
    description: &str,
    snippet: &str,
    params: Value,
) -> ToolSpec {
    ToolSpec::new(name, description)
        .label(label)
        .prompt_snippet(snippet)
        .parameters(params)
}

pub(crate) fn with_target(mut props: Vec<(&'static str, Value)>) -> Vec<(&'static str, Value)> {
    props.extend(target_props());
    props
}

pub fn specs() -> Vec<ToolSpec> {
    let mut tools = vec![
        tool(
            "chrome_launch",
            "Chrome Launch",
            "Connect to Chrome on its DevTools port or start it with the managed profile; url opens in a new tab.",
            "Connect to or start Chrome for automation.",
            schema(
                vec![
                    ("port", json!({ "type": "number" })),
                    ("url", json!({ "type": "string" })),
                    ("userDataDir", json!({ "type": "string" })),
                    ("useDefaultProfile", b("Unsupported.")),
                    ("headless", json!({ "type": "boolean" })),
                ],
                &[],
            ),
        ),
        tool(
            "chrome_tab",
            "Chrome Tab",
            "Manage tabs or show the Chrome version. Background mode opens new tabs behind and blocks activate.",
            "Manage Chrome tabs.",
            schema(
                with_target(vec![
                    (
                        "action",
                        json!({ "type": "string", "enum": ["list", "new", "activate", "close", "version"] }),
                    ),
                    ("url", s("For new.")),
                ]),
                &["action"],
            ),
        ),
        tool(
            "chrome_snapshot",
            "Chrome Snapshot",
            "Read the page title, URL, text, and visible elements with uids and coordinates.",
            "Read a Chrome page with stable element uids.",
            schema(with_target(snapshot_props()), &[]),
        ),
        tool(
            "chrome_find",
            "Chrome Find",
            "Find ranked page elements matching query.",
            "Find Chrome page elements.",
            schema(
                with_target(vec![
                    ("query", json!({ "type": "string" })),
                    ("mode", json!({ "type": "string", "enum": MODES })),
                    (
                        "maxElements",
                        json!({ "type": "number", "default": MAX_ELEMENTS }),
                    ),
                ]),
                &["query"],
            ),
        ),
        tool(
            "chrome_navigate",
            "Chrome Navigate",
            "Open a URL in a tab.",
            "Navigate a Chrome tab to a URL.",
            schema(
                with_target(vec![
                    ("url", json!({ "type": "string" })),
                    (
                        "waitUntilLoad",
                        json!({ "type": "boolean", "default": true }),
                    ),
                    ("timeoutMs", json!({ "type": "number", "default": 15000 })),
                    ("initScript", s("Runs before page scripts.")),
                ]),
                &["url"],
            ),
        ),
        tool(
            "chrome_evaluate",
            "Chrome Evaluate",
            "Run page JavaScript and return its JSON value.",
            "Run JavaScript in Chrome.",
            schema(
                with_target(vec![
                    ("expression", json!({ "type": "string" })),
                    (
                        "awaitPromise",
                        json!({ "type": "boolean", "default": true }),
                    ),
                ]),
                &["expression"],
            ),
        ),
        tool(
            "chrome_wait_for",
            "Chrome Wait For",
            "Wait until a selector exists or an expression is truthy.",
            "Wait for Chrome page state.",
            schema(
                with_target(vec![
                    (
                        "kind",
                        json!({ "type": "string", "enum": ["selector", "expression"] }),
                    ),
                    ("value", json!({ "type": "string" })),
                    ("timeoutMs", json!({ "type": "number", "default": 10000 })),
                    ("intervalMs", json!({ "type": "number", "default": 250 })),
                ]),
                &["kind", "value"],
            ),
        ),
        tool(
            "chrome_screenshot",
            "Chrome Screenshot",
            "Save a screenshot of the tab.",
            "Save Chrome screenshots.",
            schema(
                with_target(vec![
                    ("path", s("Default .pi/chrome-screenshots/")),
                    (
                        "format",
                        json!({ "type": "string", "enum": ["png", "jpeg"] }),
                    ),
                    (
                        "quality",
                        json!({ "type": "number", "minimum": 0, "maximum": 100, "description": "JPEG" }),
                    ),
                    ("fullPage", json!({ "type": "boolean" })),
                    (
                        "retentionDays",
                        json!({ "type": "number", "minimum": 0, "description": "Default 7; 0 keeps all" }),
                    ),
                ]),
                &[],
            ),
        ),
        tool(
            "chrome_cdp",
            "Chrome CDP Call",
            "Run one CDP method with params. Background mode blocks Page.bringToFront and Target.activateTarget. Results over 256 KiB are cut.",
            "Run a raw CDP method on a Chrome tab.",
            schema(
                with_target(vec![
                    ("method", json!({ "type": "string" })),
                    ("params", json!({ "type": "object" })),
                    ("timeoutMs", n("Default 5000, max 120000")),
                ]),
                &["method"],
            ),
        ),
        tool(
            "chrome_cdp_targets",
            "Chrome CDP Targets",
            "List all CDP targets: pages, workers, and extension frames.",
            "List Chrome CDP targets.",
            schema(target_props().into_iter().take(3).collect(), &[]),
        ),
    ];
    tools.extend(crate::specs_input::input_specs());
    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_match_tool_names() {
        let names: Vec<String> = specs().into_iter().map(|s| s.name).collect();
        assert_eq!(names, TOOL_NAMES);
        let snapshot = &specs()[2].parameters;
        assert!(snapshot["properties"]["targetId"].is_object());
        assert_eq!(snapshot["properties"]["mode"]["enum"][0], "auto");
    }
}
