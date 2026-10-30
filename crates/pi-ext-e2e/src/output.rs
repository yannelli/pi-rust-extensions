use std::process::ExitStatus;

use serde_json::Value;

/// What one [`crate::Run`] produced.
#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    /// The JSON events pi printed, one per stdout line.
    pub events: Vec<Value>,
    /// The chat-completions request bodies the mock provider received.
    pub requests: Vec<Value>,
    pub stdout: String,
    /// pi's stderr, with the extension log lines (`[native:e2e/<name>] ...`).
    pub stderr: String,
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect(),
        _ => String::new(),
    }
}

impl Output {
    pub fn events_of(&self, kind: &str) -> Vec<&Value> {
        self.events
            .iter()
            .filter(|event| event["type"] == kind)
            .collect()
    }

    /// `tool_execution_end` events of `tool`.
    pub fn tool_results(&self, tool: &str) -> Vec<&Value> {
        self.events_of("tool_execution_end")
            .into_iter()
            .filter(|event| event["toolName"] == tool)
            .collect()
    }

    /// The text of the first result of `tool`.
    pub fn tool_result_text(&self, tool: &str) -> Option<String> {
        self.tool_results(tool)
            .first()
            .map(|event| text_of(&event["result"]["content"]))
    }

    /// Texts of the `tool_execution_update` partial results of `tool`.
    pub fn tool_updates(&self, tool: &str) -> Vec<String> {
        let updates = self.events_of("tool_execution_update");
        let mine = updates
            .into_iter()
            .filter(|event| event["toolName"] == tool);
        mine.map(|event| text_of(&event["partialResult"]["content"]))
            .collect()
    }

    /// Tool names offered in request `index`.
    pub fn tool_names(&self, index: usize) -> Vec<String> {
        let tools = self
            .requests
            .get(index)
            .and_then(|request| request["tools"].as_array().cloned());
        let tools = tools.unwrap_or_default();
        tools
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str().map(String::from))
            .collect()
    }

    /// The system message text of request `index`.
    pub fn system_prompt(&self, index: usize) -> String {
        let messages = self
            .requests
            .get(index)
            .and_then(|request| request["messages"].as_array().cloned());
        let system = messages
            .unwrap_or_default()
            .into_iter()
            .find(|message| message["role"] == "system");
        system
            .map(|message| text_of(&message["content"]))
            .unwrap_or_default()
    }

    /// The text of the last assistant message.
    pub fn final_text(&self) -> String {
        let ends = self.events_of("message_end");
        let assistant = ends
            .into_iter()
            .rev()
            .find(|event| event["message"]["role"] == "assistant");
        assistant
            .map(|event| text_of(&event["message"]["content"]))
            .unwrap_or_default()
    }
}
