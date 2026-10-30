use pi_ext_sdk::ToolSpec;
use serde_json::{Value, json};

fn state_id() -> Value {
    json!({ "type": "string", "description": "Required state id owning every @e ref used by this operation" })
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

fn literal(value: &str) -> Value {
    json!({ "type": "string", "const": value })
}

fn union(values: &[&str], description: &str) -> Value {
    let any: Vec<Value> = values.iter().map(|v| literal(v)).collect();
    json!({ "anyOf": any, "description": description })
}

fn action_union() -> Value {
    let button = json!({ "anyOf": [literal("left"), literal("right"), literal("middle")] });
    let count = json!({ "type": "number", "minimum": 1, "maximum": 3 });
    let point = object(
        json!({ "x": { "type": "number" }, "y": { "type": "number" } }),
        &["x", "y"],
    );
    let s = |description: &str| json!({ "type": "string", "description": description });
    let any = json!([
        object(
            json!({ "action": literal("press"), "ref": s("Actionable outline ref") }),
            &["action", "ref"]
        ),
        object(
            json!({ "action": literal("click"), "ref": { "type": "string" }, "button": button, "clickCount": count }),
            &["action", "ref"]
        ),
        object(
            json!({ "action": literal("click"), "x": { "type": "number" }, "y": { "type": "number" }, "button": button, "clickCount": count }),
            &["action", "x", "y"]
        ),
        object(
            json!({ "action": literal("setText"), "ref": s("Editable outline ref"), "text": { "type": "string" } }),
            &["action", "ref", "text"]
        ),
        object(
            json!({ "action": literal("typeText"), "ref": s("Omit after a click to type into the focus established by that click"), "text": { "type": "string" } }),
            &["action", "text"]
        ),
        object(
            json!({ "action": literal("keypress"), "ref": s("Omit to send keys to the focused control"), "keys": { "type": "array", "items": { "type": "string" }, "minItems": 1 } }),
            &["action", "keys"]
        ),
        object(
            json!({ "action": literal("scroll"), "ref": { "type": "string" }, "scrollX": { "type": "number" }, "scrollY": { "type": "number" } }),
            &["action"]
        ),
        object(
            json!({ "action": literal("drag"), "path": { "type": "array", "items": point, "minItems": 2 } }),
            &["action", "path"]
        ),
        object(
            json!({ "action": literal("moveMouse"), "x": { "type": "number" }, "y": { "type": "number" } }),
            &["action", "x", "y"]
        ),
    ]);
    json!({ "anyOf": any })
}

fn condition_properties() -> serde_json::Map<String, Value> {
    let s = |description: &str, max: u64| json!({ "type": "string", "description": description, "maxLength": max });
    let value = json!({
        "ref": s("Specific @e ref to test", 128),
        "scopeRef": s("Restrict matching to this @e subtree", 128),
        "text": s("Text that must match", 512),
        "role": s("Exact normalized role", 128),
        "value": { "type": "string", "description": "Exact normalized value; normally pair with ref" },
        "until": union(&["present", "absent"], "Desired condition, default present"),
        "timeoutMs": { "type": "number", "description": "Maximum wait, default 10000ms", "minimum": 100, "maximum": 60000 },
    });
    value.as_object().cloned().unwrap_or_default()
}

pub fn find_roots() -> ToolSpec {
    let s = |description: &str| json!({ "type": "string", "description": description, "maxLength": 256 });
    let kinds = [
        "window",
        "menu",
        "sheet",
        "popover",
        "dialog",
        "browser_page",
    ];
    ToolSpec::new(
        "find_roots",
        "Find a bounded, ranked set of controllable UI roots with refs, geometry, and focus state.",
    )
    .label("Find Roots")
    .prompt_snippet("Find a target root before observe_ui when needed.")
    .parameters(object(
        json!({
            "text": s("Ranked app or title text"),
            "app": s("Exact normalized app name"),
            "bundleId": { "type": "string", "description": "Exact bundle id" },
            "pid": { "type": "number", "description": "Exact process id" },
            "kind": union(&kinds, "Exact root kind"),
        }),
        &[],
    ))
}

pub fn observe_ui() -> ToolSpec {
    ToolSpec::new("observe_ui", "Capture the current/frontmost root or one exact @r root and return a bounded UI outline.")
        .label("Observe UI")
        .prompt_snippet("Primary UI observation tool. Follow with search_ui, expand_ui, inspect_ui, or act_ui.")
        .guideline("Use mode=semantic to skip OCR and images, visual to force them, and fused for automatic selection.")
        .guideline("Use @e outline refs from observe_ui/search_ui for act_ui; pictureOnly refs are coordinate-only and blocked by UI-tree-only policy.")
        .parameters(object(
            json!({
                "root": { "type": "string", "description": "Exact @r ref issued by find_roots" },
                "mode": union(&["semantic", "visual", "fused"], "Observation mode, default fused"),
            }),
            &[],
        ))
}

pub fn search_ui() -> ToolSpec {
    let s = |description: &str, max: u64| json!({ "type": "string", "description": description, "maxLength": max });
    ToolSpec::new("search_ui", "Return a bounded, deterministically ranked search of the cached outline. At least one predicate is required.")
        .label("Search UI")
        .prompt_snippet("Find targets not shown in the compact observe_ui output; refine broad searches instead of paging matches.")
        .parameters(object(
            json!({
                "text": s("Human-readable text or label", 256),
                "role": s("Exact normalized role, e.g. button", 128),
                "capability": s("Exact capability, e.g. press", 128),
                "stateId": state_id(),
            }),
            &["stateId"],
        ))
}

pub fn expand_ui() -> ToolSpec {
    let depth = json!({ "type": "number", "minimum": 1, "maximum": 8, "description": "Subtree depth, default 3" });
    ToolSpec::new(
        "expand_ui",
        "Unfold bounded local outline context for one @e ref.",
    )
    .label("Expand UI")
    .prompt_snippet("Expand a specific ref instead of dumping unrelated UI.")
    .parameters(object(
        json!({ "ref": { "type": "string" }, "depth": depth, "stateId": state_id() }),
        &["ref", "stateId"],
    ))
}

pub fn inspect_ui() -> ToolSpec {
    ToolSpec::new(
        "inspect_ui",
        "Inspect one exact outline ref with fields, geometry, capabilities, and annotations.",
    )
    .label("Inspect UI")
    .prompt_snippet("Use when a target's evidence or provenance matters.")
    .parameters(object(
        json!({ "ref": { "type": "string" }, "stateId": state_id() }),
        &["ref", "stateId"],
    ))
}

pub fn act_ui() -> ToolSpec {
    let mut expect = serde_json::Map::new();
    expect.insert("type".into(), json!("object"));
    expect.insert("properties".into(), Value::Object(condition_properties()));
    let actions =
        json!({ "type": "array", "items": action_union(), "minItems": 1, "maxItems": 20 });
    ToolSpec::new("act_ui", "Perform one or more precisely targeted checked actions and return the successor state.")
        .label("Act")
        .prompt_snippet("Pass dependent click/type steps together and use expect for observable completion.")
        .guideline("After clicking an editable region, omit ref from typeText/keypress so input follows the established focus.")
        .parameters(object(json!({ "stateId": state_id(), "expect": expect, "actions": actions }), &["stateId", "actions"]))
}

pub fn read_text() -> ToolSpec {
    ToolSpec::new("read_text", "Read a fixed-size page from an @e UI ref or immutable @o truncated-output ref.")
        .label("Read Text")
        .prompt_snippet("Use @e with its stateId; @o continuation refs don't need stateId.")
        .parameters(object(
            json!({ "ref": { "type": "string" }, "offset": { "type": "number", "minimum": 0 }, "stateId": state_id() }),
            &["ref"],
        ))
}

pub fn wait_for() -> ToolSpec {
    let mut properties = condition_properties();
    properties.insert("stateId".into(), state_id());
    ToolSpec::new(
        "wait_for",
        "Wait for one scoped UI condition and return the successor state.",
    )
    .label("Wait For")
    .prompt_snippet("Use after asynchronous UI changes instead of polling observe_ui.")
    .parameters(object(Value::Object(properties), &["stateId"]))
}

pub fn launch_browser() -> ToolSpec {
    ToolSpec::new(
        "launch_browser",
        "Launch the configured Pi-managed CDP browser and return an observed browser-page state.",
    )
    .label("Launch Browser Context")
    .prompt_snippet("Use for browser work that needs a managed CDP context.")
    .guideline("Prefer curl through bash when the page is directly fetchable.")
    .parameters(object(
        json!({ "url": { "type": "string", "maxLength": 8192 } }),
        &[],
    ))
}

pub fn navigate_browser() -> ToolSpec {
    ToolSpec::new(
        "navigate_browser",
        "Navigate an observed CDP browser-page state to an HTTP(S) URL.",
    )
    .label("Navigate Browser")
    .prompt_snippet("Native browser windows use act_ui; this tool is CDP-only.")
    .parameters(object(
        json!({ "url": { "type": "string", "maxLength": 8192 }, "stateId": state_id() }),
        &["url", "stateId"],
    ))
}

pub fn evaluate_browser() -> ToolSpec {
    ToolSpec::new("evaluate_browser", "Evaluate targeted JavaScript in a CDP browser-page state; returned output is strictly bounded.")
        .label("Evaluate Browser")
        .prompt_snippet("Prefer observe/search/read; return selected fields, aggregates, or bounded slices.")
        .parameters(object(
            json!({ "stateId": state_id(), "expression": { "type": "string", "maxLength": 65536 } }),
            &["stateId", "expression"],
        ))
}
