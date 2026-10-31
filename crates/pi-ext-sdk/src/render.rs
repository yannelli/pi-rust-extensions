//! Custom renderers: pi asks for the lines of a tool row, a custom message, or a custom session
//! entry at one width (`render.*` requests). pi caches the answer per width and never waits for it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The render context of a tool row (upstream `ToolRenderContext` without its functions).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolRenderContext {
    pub args: Value,
    pub tool_call_id: Option<String>,
    pub cwd: Option<String>,
    pub execution_started: bool,
    pub args_complete: bool,
    pub is_partial: bool,
    pub expanded: bool,
    pub show_images: bool,
    pub is_error: bool,
}

/// `render.toolCall`: the call row of a tool with [`crate::Extension::render_tool_call`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallRender {
    pub tool_name: String,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub args: Value,
    #[serde(default)]
    pub context: ToolRenderContext,
    pub width: usize,
}

/// `options` of `render.toolResult`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct ResultOptions {
    pub expanded: bool,
    pub is_partial: bool,
}

/// `render.toolResult`: the result row; `result` has `content`, `details`, and `isError`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultRender {
    pub tool_name: String,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub result: Value,
    #[serde(default)]
    pub options: ResultOptions,
    #[serde(default)]
    pub context: ToolRenderContext,
    pub width: usize,
}

impl ToolResultRender {
    /// The text blocks of `result.content`, joined.
    pub fn text(&self) -> String {
        let blocks = self.result.get("content").and_then(Value::as_array);
        let texts = blocks.into_iter().flatten();
        texts
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect()
    }
}

/// `options` of `render.message` and `render.entry`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ContentOptions {
    pub expanded: bool,
}

/// `render.message`: a custom message of a type the extension draws.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MessageRender {
    pub custom_type: String,
    #[serde(default)]
    pub message: Value,
    #[serde(default)]
    pub options: ContentOptions,
    pub width: usize,
}

/// `render.entry`: a custom session entry of a type the extension draws.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EntryRender {
    pub custom_type: String,
    #[serde(default)]
    pub entry: Value,
    #[serde(default)]
    pub options: ContentOptions,
    pub width: usize,
}

impl EntryRender {
    /// `entry.data` as `T`, or `None` when it does not match.
    pub fn data<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
        serde_json::from_value(self.entry.get("data")?.clone()).ok()
    }
}

/// The scope of `settings.set`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SettingsScope {
    /// `<agentDir>/settings.json`.
    #[default]
    Global,
    /// `<cwd>/.pi/settings.json`; pi refuses it for an untrusted project.
    Project,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_host_shapes() {
        let params = json!({
            "toolName": "t", "toolCallId": "c1", "width": 40, "options": { "isPartial": true },
            "result": { "content": [{ "type": "text", "text": "a" }, { "type": "text", "text": "b" }] },
            "context": { "argsComplete": true, "unknown": 1 }
        });
        let request: ToolResultRender = serde_json::from_value(params).unwrap();
        assert_eq!((request.text(), request.width), ("ab".into(), 40));
        assert!(request.options.is_partial && request.context.args_complete);
        let entry: EntryRender = serde_json::from_value(json!({
            "customType": "note", "entry": { "data": { "n": 2 } }, "width": 10
        }))
        .unwrap();
        assert_eq!(entry.data::<Value>(), Some(json!({ "n": 2 })));
    }
}
