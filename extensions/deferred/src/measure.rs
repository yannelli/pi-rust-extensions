//! Token estimates and text helpers shared by the plan and the `/deferred` panel.

use pi_ext_sdk::style;
use serde_json::Value;

pub fn tokens(chars: usize) -> usize {
    chars.div_ceil(4)
}

pub fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// Extension and MCP tools, which `auto` may defer.
pub fn is_addon(tool: &Value) -> bool {
    let source = tool.get("sourceInfo");
    let path = source.map_or("", |s| str_of(s, "path"));
    let origin = source.map_or("", |s| str_of(s, "source"));
    path == "builtin:mcp" || (!origin.is_empty() && origin != "builtin")
}

/// pi keeps `codemode` and `deferred` exposure tools out of the declarations by itself.
pub fn is_undeclared(tool: &Value) -> bool {
    matches!(str_of(tool, "exposure"), "codemode" | "deferred")
}

pub fn tool_tokens(tool: &Value) -> usize {
    let schema = tool.get("parameters").map_or(0, |p| p.to_string().len());
    tokens(str_of(tool, "name").len() + str_of(tool, "description").len() + schema)
}

pub fn skill_tokens(skill: &Value) -> usize {
    let (name, description) = (str_of(skill, "name"), str_of(skill, "description"));
    tokens(name.len() + description.len() + str_of(skill, "filePath").len() + 60)
}

pub fn context_tokens(path: &str, content: &str) -> usize {
    tokens(path.len() + content.len() + 50)
}

pub fn section_tokens(name: &str, text: &str) -> usize {
    tokens(name.len() * 2 + text.len() + 6)
}

/// The first non-empty line without heading marks, at most 120 characters.
pub fn summary_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty());
    let line = line.unwrap_or_default().trim_start_matches('#').trim();
    match line.char_indices().nth(120) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_owned(),
    }
}

/// Words of `text` wrapped to `width` columns, at least one line.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split_whitespace() {
        let last = lines.last_mut().expect("one line");
        if !last.is_empty() && style::visible_width(last) + 1 + style::visible_width(word) > width {
            lines.push(word.to_owned());
        } else {
            if !last.is_empty() {
                last.push(' ');
            }
            last.push_str(word);
        }
    }
    lines
}
