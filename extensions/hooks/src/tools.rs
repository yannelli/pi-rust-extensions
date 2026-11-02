use std::path::{Component, Path, PathBuf};

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

/// Codex names a Codex matcher tests for this Claude name: `apply_patch` and its aliases.
pub fn codex_aliases(claude: &str) -> &'static [&'static str] {
    match claude {
        "Edit" | "Write" => &["apply_patch", "Edit", "Write"],
        "Agent" => &["spawn_agent"],
        _ => &[],
    }
}

/// Codex hook `tool_name`: file edits go through `apply_patch`.
pub fn codex_tool_name(claude: &str) -> String {
    match claude {
        "Edit" | "Write" => "apply_patch".into(),
        other => other.into(),
    }
}

/// Codex `tool_input` for pi edit and write: `{ command: <apply_patch text> }`.
pub fn codex_tool_input(pi_tool: &str, hook_input: &Value) -> Value {
    let path = hook_input.get("file_path").and_then(Value::as_str);
    let lines = |text: &str, sign: char| -> String {
        text.lines().map(|line| format!("{sign}{line}\n")).collect()
    };
    let body = match (pi_tool, path) {
        ("write", Some(path)) => {
            let content = hook_input
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            format!("*** Add File: {path}\n{}", lines(content, '+'))
        }
        ("edit", Some(path)) => {
            let mut body = format!("*** Update File: {path}\n");
            for edit in hook_input
                .get("edits")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let old = edit
                    .get("old_string")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let new = edit
                    .get("new_string")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                body.push_str(&format!("@@\n{}{}", lines(old, '-'), lines(new, '+')));
            }
            body
        }
        _ => return hook_input.clone(),
    };
    json!({ "command": format!("*** Begin Patch\n{body}*** End Patch\n") })
}

/// `~` and relative paths -> absolute, so path guards see one spelling.
pub fn absolute_path(path: &str, cwd: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let resolved = match (path.strip_prefix('~'), home) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => {
            home.join(rest.trim_start_matches('/'))
        }
        _ => cwd.join(path),
    };
    let mut out = PathBuf::new();
    for part in resolved.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out.display().to_string()
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

/// pi input -> hook `tool_input`: adds Claude key names (absolute `file_path`) next to the pi names.
pub fn to_hook_input(pi_tool: &str, input: &Value, cwd: &Path) -> Value {
    let mut out = input.clone();
    let Some(map) = out.as_object_mut() else {
        return out;
    };
    if matches!(pi_tool, "read" | "edit" | "write")
        && let Some(path) = map.get("path").and_then(Value::as_str)
    {
        let path = absolute_path(path, cwd);
        map.insert("file_path".into(), json!(path));
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
        let pi = json!({ "path": "/r/a.ts", "edits": [{ "oldText": "x", "newText": "y" }] });
        let hook = to_hook_input("edit", &pi, Path::new("/r"));
        assert_eq!(hook["file_path"], "/r/a.ts");
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
        let cwd = Path::new("/r/sub");
        let hook = to_hook_input("read", &json!({ "path": "../f", "offset": 2 }), cwd);
        assert_eq!(
            hook,
            json!({ "path": "../f", "offset": 2, "file_path": "/r/f" })
        );
        assert_eq!(
            from_hook_input("read", &hook),
            json!({ "path": "/r/f", "offset": 2 })
        );
        let bash = json!({ "command": "ls" });
        assert_eq!(to_hook_input("bash", &bash, cwd), bash);
        assert_eq!(primary_input("Bash", &bash), Some("ls"));
    }

    #[test]
    fn codex_apply_patch_input() {
        let cwd = Path::new("/r");
        let write = to_hook_input("write", &json!({ "path": "n.txt", "content": "a\nb" }), cwd);
        assert_eq!(
            codex_tool_input("write", &write)["command"],
            "*** Begin Patch\n*** Add File: /r/n.txt\n+a\n+b\n*** End Patch\n"
        );
        let pi = json!({ "path": "e.rs", "edits": [{ "oldText": "x", "newText": "y" }] });
        let edit = codex_tool_input("edit", &to_hook_input("edit", &pi, cwd));
        assert_eq!(
            edit["command"],
            "*** Begin Patch\n*** Update File: /r/e.rs\n@@\n-x\n+y\n*** End Patch\n"
        );
        assert_eq!(codex_tool_name("Edit"), "apply_patch");
        assert_eq!(codex_tool_name("Bash"), "Bash");
    }
}
