use serde_json::{Map, Value, json};

/// pi tool name -> hook tool name. Other names (MCP tools too) stay as they are.
pub fn hook_tool_name(pi: &str) -> String {
    let name = match pi {
        "bash" => "Bash",
        "read" => "Read",
        "edit" => "Edit",
        "write" => "Write",
        "grep" => "Grep",
        "find" => "Glob",
        "ls" => "LS",
        "powershell" => "PowerShell",
        other => other,
    };
    name.to_owned()
}

/// Codex tool names that a Codex matcher also tests for this Claude name.
pub fn codex_aliases(claude: &str) -> &'static [&'static str] {
    match claude {
        "Bash" => &["shell", "exec_command", "local_shell"],
        "Edit" | "Write" => &["apply_patch"],
        _ => &[],
    }
}

/// The value an `if` rule pattern is tested against.
pub fn primary_input<'a>(tool: &str, input: &'a Value) -> Option<&'a str> {
    let keys: &[&str] = match tool {
        "Bash" | "PowerShell" => &["command"],
        "Read" | "Edit" | "Write" | "LS" => &["file_path", "path"],
        "Glob" | "Grep" => &["pattern"],
        _ => &[],
    };
    keys.iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
}

/// pi input -> hook `tool_input`: adds Claude key names next to the pi names.
pub fn to_hook_input(pi_tool: &str, input: &Value) -> Value {
    let mut out = input.clone();
    let Some(map) = out.as_object_mut() else {
        return out;
    };
    if matches!(pi_tool, "read" | "edit" | "write")
        && let Some(path) = map.get("path").cloned()
    {
        map.insert("file_path".into(), path);
    }
    if pi_tool == "edit"
        && let Some(edits) = map.get("edits").and_then(Value::as_array).cloned()
    {
        let mapped: Vec<Value> = edits
            .iter()
            .map(|e| rename(e, &[("oldText", "old_string"), ("newText", "new_string")]))
            .collect();
        if let [single] = mapped.as_slice() {
            for key in ["old_string", "new_string"] {
                if let Some(value) = single.get(key) {
                    map.insert(key.into(), value.clone());
                }
            }
        }
        map.insert("edits".into(), Value::Array(mapped));
    }
    out
}

/// Hook `updatedInput` (whole object) -> pi input.
pub fn from_hook_input(pi_tool: &str, hook: &Value) -> Value {
    let mut out = hook.clone();
    let Some(map) = out.as_object_mut() else {
        return out;
    };
    if matches!(pi_tool, "read" | "edit" | "write")
        && let Some(path) = map.remove("file_path")
    {
        map.insert("path".into(), path);
    }
    if pi_tool == "edit" {
        let old = map.remove("old_string");
        let new = map.remove("new_string");
        map.remove("replace_all");
        let edits: Option<Vec<Value>> = map.get("edits").and_then(Value::as_array).map(|edits| {
            edits
                .iter()
                .map(|e| rename(e, &[("old_string", "oldText"), ("new_string", "newText")]))
                .collect()
        });
        let edits = match (old, new, edits) {
            (Some(old), Some(new), Some(edits)) if edits.len() <= 1 => {
                vec![json!({ "oldText": old, "newText": new })]
            }
            (Some(old), Some(new), None) => vec![json!({ "oldText": old, "newText": new })],
            (_, _, edits) => edits.unwrap_or_default(),
        };
        if !edits.is_empty() {
            map.insert("edits".into(), Value::Array(edits));
        }
    }
    out
}

fn rename(value: &Value, pairs: &[(&str, &str)]) -> Value {
    let Some(map) = value.as_object() else {
        return value.clone();
    };
    let renamed: Map<String, Value> = map
        .iter()
        .map(|(key, v)| {
            let key = pairs
                .iter()
                .find(|(from, _)| from == key)
                .map_or(key.as_str(), |(_, to)| to);
            (key.to_owned(), v.clone())
        })
        .collect();
    Value::Object(renamed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_map() {
        assert_eq!(hook_tool_name("bash"), "Bash");
        assert_eq!(hook_tool_name("find"), "Glob");
        assert_eq!(hook_tool_name("ls"), "LS");
        assert_eq!(hook_tool_name("mcp__fs__read"), "mcp__fs__read");
    }

    #[test]
    fn edit_input_round_trips() {
        let pi = json!({ "path": "a.ts", "edits": [{ "oldText": "x", "newText": "y" }] });
        let hook = to_hook_input("edit", &pi);
        assert_eq!(hook["file_path"], "a.ts");
        assert_eq!(hook["old_string"], "x");
        assert_eq!(hook["edits"][0]["new_string"], "y");
        assert_eq!(from_hook_input("edit", &hook), pi);
        let changed = json!({ "file_path": "b.ts", "old_string": "x", "new_string": "z" });
        assert_eq!(
            from_hook_input("edit", &changed),
            json!({ "path": "b.ts", "edits": [{ "oldText": "x", "newText": "z" }] })
        );
    }

    #[test]
    fn read_and_bash_inputs() {
        let hook = to_hook_input("read", &json!({ "path": "f", "offset": 2 }));
        assert_eq!(hook, json!({ "path": "f", "offset": 2, "file_path": "f" }));
        assert_eq!(
            from_hook_input("read", &hook),
            json!({ "path": "f", "offset": 2 })
        );
        let bash = json!({ "command": "ls" });
        assert_eq!(to_hook_input("bash", &bash), bash);
        assert_eq!(primary_input("Bash", &bash), Some("ls"));
    }
}
