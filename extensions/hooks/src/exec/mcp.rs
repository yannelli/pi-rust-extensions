//! `mcp_tool` handlers: call a tool of a connected MCP server through pi's `mcp.callTool`.
use std::time::Duration;

use pi_ext_sdk::Host;
use serde_json::{Map, Value, json};

use crate::output::RawRun;

/// The value at a dotted path such as `tool_input.file_path`.
fn lookup<'a>(event: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(event, |value, key| match value {
        Value::Array(items) => key.parse::<usize>().ok().and_then(|i| items.get(i)),
        other => other.get(key),
    })
}

fn as_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// `${a.b}` filling a whole string keeps the JSON type; inside text it renders as text.
pub fn expand(template: &Value, event: &Value) -> Value {
    match template {
        Value::String(text) => {
            let whole = text
                .strip_prefix("${")
                .and_then(|t| t.strip_suffix('}'))
                .filter(|path| !path.contains(['$', '{', '}']));
            if let Some(path) = whole {
                return lookup(event, path).cloned().unwrap_or(Value::Null);
            }
            Value::String(interpolate(text, event))
        }
        Value::Array(items) => Value::Array(items.iter().map(|v| expand(v, event)).collect()),
        Value::Object(map) => {
            let expanded: Map<String, Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), expand(v, event)))
                .collect();
            Value::Object(expanded)
        }
        other => other.clone(),
    }
}

fn interpolate(text: &str, event: &Value) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        match after.find('}') {
            Some(end) => {
                out.push_str(
                    &lookup(event, &after[..end])
                        .map(as_text)
                        .unwrap_or_default(),
                );
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[at..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// The text blocks of an MCP result, joined like command stdout.
pub fn result_text(result: &Value) -> String {
    let texts: Vec<&str> = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect();
    texts.join("\n")
}

pub async fn run_mcp_tool(host: &Host, handler: &Value, input: &str, timeout: Duration) -> RawRun {
    let field = |key: &str| {
        handler
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let (Some(server), Some(tool)) = (field("server"), field("tool")) else {
        return RawRun::failed("mcp_tool hook needs server and tool");
    };
    let event: Value = serde_json::from_str(input).unwrap_or(Value::Null);
    let args = expand(handler.get("input").unwrap_or(&json!({})), &event);
    let params = json!({ "server": server, "tool": tool, "args": args, "timeout": timeout.as_millis() as u64 });
    match tokio::time::timeout(timeout, host.call("mcp.callTool", params)).await {
        Err(_) => RawRun::failed(format!("timed out after {}s", timeout.as_secs_f64())),
        Ok(Err(err)) => RawRun::failed(format!("mcp.callTool {server}/{tool} failed: {err}")),
        Ok(Ok(result)) if result.get("isError") == Some(&Value::Bool(true)) => RawRun::exit(
            1,
            "",
            format!(
                "{server}/{tool} returned an error: {}",
                result_text(&result)
            ),
        ),
        Ok(Ok(result)) => RawRun::exit(0, result_text(&result), ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_keep_types_and_render_text() {
        let event =
            json!({ "tool_input": { "file_path": "src/main.rs", "count": 3 }, "list": [1, 2] });
        let template = json!({
            "path": "${tool_input.file_path}", "count": "${tool_input.count}",
            "message": "Scanning ${tool_input.file_path} (${tool_input.count})",
            "nested": ["${list.1}", { "missing": "${nope.x}" }],
        });
        assert_eq!(
            expand(&template, &event),
            json!({ "path": "src/main.rs", "count": 3, "message": "Scanning src/main.rs (3)",
                "nested": [2, { "missing": null }] })
        );
    }

    #[test]
    fn text_blocks_join() {
        let result = json!({ "content": [{ "type": "text", "text": "a" }, { "type": "image" }, { "type": "text", "text": "b" }] });
        assert_eq!(result_text(&result), "a\nb");
    }
}
