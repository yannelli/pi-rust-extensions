use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::json::merge_patch_compose;

/// Content block helpers for tool results and messages.
pub mod content {
    use serde_json::{Value, json};

    pub fn text(text: impl Into<String>) -> Value {
        json!({ "type": "text", "text": text.into() })
    }

    pub fn image(base64: impl Into<String>, mime_type: impl Into<String>) -> Value {
        json!({ "type": "image", "data": base64.into(), "mimeType": mime_type.into() })
    }
}

/// The answer to `tool.execute`. `From<String>` and `From<&str>` build a text result.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub content: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
}

impl ToolResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self::content(vec![content::text(text)])
    }

    pub fn content(content: Vec<Value>) -> Self {
        Self {
            content,
            ..Self::default()
        }
    }

    /// A text result that pi shows as a failed tool call.
    pub fn error(text: impl Into<String>) -> Self {
        Self::text(text).failed()
    }

    pub fn details(mut self, details: impl Serialize) -> Self {
        self.details = serde_json::to_value(details).ok();
        self
    }

    pub fn failed(mut self) -> Self {
        self.is_error = true;
        self
    }

    pub(crate) fn partial_json(&self) -> Value {
        let mut partial = json!({ "content": self.content });
        if let Some(details) = &self.details {
            partial["details"] = details.clone();
        }
        partial
    }
}

impl From<String> for ToolResult {
    fn from(text: String) -> Self {
        Self::text(text)
    }
}

impl From<&str> for ToolResult {
    fn from(text: &str) -> Self {
        Self::text(text)
    }
}

/// The answer to `event`: a handler `result` and an RFC 7386 `patch` for the event object.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct EventReply {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<Value>,
}

impl EventReply {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn result(result: impl Serialize) -> Self {
        Self::none().with_result(result)
    }

    pub fn patch(patch: Value) -> Self {
        Self::none().with_patch(patch)
    }

    /// `tool_call`: block the call with a reason.
    pub fn block(reason: impl Into<String>) -> Self {
        Self::result(json!({ "block": true, "reason": reason.into() }))
    }

    /// `input`: replace the input text.
    pub fn transform(text: impl Into<String>) -> Self {
        Self::result(json!({ "action": "transform", "text": text.into() }))
    }

    /// `input`: the extension handled the input; pi does not send it.
    pub fn handled() -> Self {
        Self::result(json!({ "action": "handled" }))
    }

    /// `session_before_*`: cancel the operation.
    pub fn cancel() -> Self {
        Self::result(json!({ "cancel": true }))
    }

    /// `before_agent_start`: set `systemPromptOptions.sections.<key>`.
    pub fn section(key: &str, text: impl Into<String>) -> Self {
        Self::none().with_section(key, Some(text.into()))
    }

    /// `before_agent_start`: set (`Some`) or remove (`None`) one system prompt section.
    pub fn with_section(self, key: &str, text: Option<String>) -> Self {
        let mut sections = Map::new();
        sections.insert(key.to_owned(), text.map_or(Value::Null, Value::String));
        self.with_patch(json!({ "systemPromptOptions": { "sections": sections } }))
    }

    pub fn with_result(mut self, result: impl Serialize) -> Self {
        self.result = serde_json::to_value(result).ok().filter(|v| !v.is_null());
        self
    }

    /// Merges `patch` into the patch already present; `null` values stay (they delete keys).
    pub fn with_patch(mut self, patch: Value) -> Self {
        self.patch = Some(match self.patch.take() {
            Some(mut base) => {
                merge_patch_compose(&mut base, patch);
                base
            }
            None => patch,
        });
        self
    }
}

impl From<()> for EventReply {
    fn from((): ()) -> Self {
        Self::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_patches_compose() {
        let reply = EventReply::section("a", "x").with_section("b", None);
        assert_eq!(
            serde_json::to_value(&reply).unwrap(),
            json!({ "patch": { "systemPromptOptions": { "sections": { "a": "x", "b": null } } } })
        );
    }

    #[test]
    fn tool_result_shape() {
        let result = ToolResult::error("boom").details(json!({ "n": 1 }));
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({ "content": [{ "type": "text", "text": "boom" }], "details": { "n": 1 }, "isError": true })
        );
    }
}
