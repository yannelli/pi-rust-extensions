//! Typed views of common pi events for `EventCtx::event_as`. Unknown fields land in `extra`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolCallEvent {
    pub tool_call_id: String,
    pub tool_name: String,
    pub input: Value,
    pub parent_tool_call_id: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolResultEvent {
    pub tool_call_id: String,
    pub tool_name: String,
    pub input: Value,
    pub content: Vec<Value>,
    pub details: Option<Value>,
    pub is_error: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InputEvent {
    pub text: String,
    pub images: Vec<Value>,
    /// `interactive`, `rpc`, or `extension`.
    pub source: String,
    pub streaming_behavior: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BeforeAgentStartEvent {
    pub system_prompt: String,
    pub system_prompt_options: Value,
    /// `user` or `custom`.
    pub message_type: Option<String>,
    pub prompt: Option<String>,
    pub message: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionStartEvent {
    /// `startup`, `reload`, `new`, `resume`, or `fork`.
    pub reason: String,
    pub previous_session_file: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionShutdownEvent {
    /// `quit`, `reload`, `new`, `resume`, or `fork`.
    pub reason: String,
    pub target_session_file: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
