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

const BACKGROUND: &str = "Ignored while background mode is on. With /chrome background off, false brings the tab to the front.";

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
        (
            "targetId",
            s("CDP target id from chrome_tab list. Default: this session's own tab."),
        ),
        (
            "urlIncludes",
            s("Pick the tab whose URL contains this text."),
        ),
        (
            "titleIncludes",
            s("Pick the tab whose title contains this text."),
        ),
        ("background", b(BACKGROUND)),
    ]
}

pub(crate) fn point_props() -> Vec<(&'static str, Value)> {
    vec![
        (
            "uid",
            s("Element uid from chrome_snapshot. Prefer it to selector."),
        ),
        ("selector", s("CSS selector.")),
        ("x", n("Viewport x when uid and selector are not set.")),
        ("y", n("Viewport y when uid and selector are not set.")),
    ]
}

pub(crate) fn schema(props: Vec<(&'static str, Value)>, required: &[&str]) -> Value {
    let mut properties = Map::new();
    for (key, value) in props {
        properties.insert(key.to_owned(), value);
    }
    json!({ "type": "object", "properties": properties, "required": required })
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
            "Connect to Chrome on the DevTools port, or start Chrome with the managed profile. Optionally open a URL in a new tab of this session.",
            "Connect to or start Chrome for automation.",
            schema(
                vec![
                    ("port", n("DevTools port. Default from config (9222).")),
                    ("url", s("URL to open in a new tab.")),
                    (
                        "userDataDir",
                        s("Profile folder for a launched Chrome. Default: the managed profile."),
                    ),
                    (
                        "useDefaultProfile",
                        b("Not supported: Chrome blocks remote debugging of the default profile."),
                    ),
                    ("headless", b("Start the managed Chrome without a window.")),
                ],
                &[],
            ),
        ),
        tool(
            "chrome_tab",
            "Chrome Tab",
            "List, open, activate, or close Chrome tabs, or show the Chrome version. New tabs belong to this session. Background mode keeps new tabs in the background and blocks activate.",
            "List, open, activate, and close Chrome tabs.",
            schema(
                with_target(vec![
                    (
                        "action",
                        json!({ "type": "string", "enum": ["list", "new", "activate", "close", "version"] }),
                    ),
                    ("url", s("URL for action=new.")),
                ]),
                &["action"],
            ),
        ),
        tool(
            "chrome_snapshot",
            "Chrome Snapshot",
            "Read the page: title, URL, visible elements with stable uids and coordinates, and page text. Use mode, query, or nearUid to narrow the output.",
            "Read a Chrome page with stable element uids.",
            schema(with_target(snapshot_props()), &[]),
        ),
        tool(
            "chrome_find",
            "Chrome Find",
            "Find elements on the page that match a query. Returns ranked elements with uids.",
            "Find Chrome page elements by text.",
            schema(
                with_target(vec![
                    ("query", s("What to find, for example 'search box'.")),
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
            "Open a URL in a Chrome tab. With no target, it uses this session's own tab and does not replace the user's tabs.",
            "Navigate a Chrome tab to a URL.",
            schema(
                with_target(vec![
                    ("url", json!({ "type": "string" })),
                    (
                        "waitUntilLoad",
                        json!({ "type": "boolean", "default": true }),
                    ),
                    ("timeoutMs", json!({ "type": "number", "default": 15000 })),
                    (
                        "initScript",
                        s("JavaScript that runs before page scripts on the next document."),
                    ),
                ]),
                &["url"],
            ),
        ),
        tool(
            "chrome_evaluate",
            "Chrome Evaluate",
            "Run JavaScript in the page and return the JSON value.",
            "Run JavaScript in a Chrome tab.",
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
            "Wait until a CSS selector exists or a JavaScript expression is truthy.",
            "Wait for page state in Chrome.",
            schema(
                with_target(vec![
                    (
                        "kind",
                        json!({ "type": "string", "enum": ["selector", "expression"] }),
                    ),
                    ("value", s("CSS selector or JavaScript expression.")),
                    ("timeoutMs", json!({ "type": "number", "default": 10000 })),
                    ("intervalMs", json!({ "type": "number", "default": 250 })),
                ]),
                &["kind", "value"],
            ),
        ),
        tool(
            "chrome_screenshot",
            "Chrome Screenshot",
            "Capture the tab and save the image. Default folder: .pi/chrome-screenshots.",
            "Save Chrome screenshots.",
            schema(
                with_target(vec![
                    (
                        "path",
                        s("Output path. Default .pi/chrome-screenshots/<time>.<format>."),
                    ),
                    (
                        "format",
                        json!({ "type": "string", "enum": ["png", "jpeg"] }),
                    ),
                    (
                        "quality",
                        json!({ "type": "number", "minimum": 0, "maximum": 100, "description": "JPEG quality." }),
                    ),
                    ("fullPage", b("Capture the full page height.")),
                    (
                        "retentionDays",
                        json!({ "type": "number", "minimum": 0, "description": "Delete default-folder captures older than this. The newest 20 stay. Default 7; 0 keeps all." }),
                    ),
                ]),
                &[],
            ),
        ),
        tool(
            "chrome_cdp",
            "Chrome CDP Call",
            "Run one Chrome DevTools Protocol method on a tab, for example Network.getCookies. Put CDP fields in params. No method filter applies except that background mode blocks Page.bringToFront and Target.activateTarget. Results over 256 KiB are cut.",
            "Run a raw CDP method on a Chrome tab.",
            schema(
                with_target(vec![
                    ("method", s("CDP method, for example Runtime.evaluate.")),
                    (
                        "params",
                        json!({ "type": "object", "additionalProperties": true }),
                    ),
                    ("timeoutMs", n("Default 5000, maximum 120000.")),
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
