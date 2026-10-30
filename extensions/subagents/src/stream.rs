//! Folds the child's `--mode json` event stream into run progress.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub done: bool,
    pub is_error: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub turns: u32,
    pub tools: Vec<ToolCall>,
    /// Text of the assistant message in progress, or of the last one.
    pub text: String,
    pub final_text: Option<String>,
    pub error: Option<String>,
    pub usage: Usage,
    pub model: Option<String>,
    pub settled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    None,
    Changed,
    /// A successful child `todo` call; holds its result `details`.
    Todo(Value),
}

impl Progress {
    pub fn apply_line(&mut self, line: &str) -> Effect {
        match serde_json::from_str::<Value>(line.trim()) {
            Ok(event) => self.apply(&event),
            Err(_) => Effect::None,
        }
    }

    pub fn apply(&mut self, event: &Value) -> Effect {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "turn_start" => self.turns += 1,
            "message_start" if role(&event["message"]) == "assistant" => self.text.clear(),
            "message_update" => {
                let update = &event["assistantMessageEvent"];
                if update["type"] != "text_delta" {
                    return Effect::None;
                }
                self.text.push_str(update["delta"].as_str().unwrap_or(""));
            }
            "message_end" if role(&event["message"]) == "assistant" => {
                self.end_message(&event["message"])
            }
            "tool_execution_start" => self.tools.push(ToolCall {
                id: str_of(event, "toolCallId"),
                name: str_of(event, "toolName"),
                summary: summarize(&event["args"]),
                done: false,
                is_error: false,
            }),
            "tool_execution_end" => return self.end_tool(event),
            "agent_end" => {
                let messages = event["messages"].as_array().cloned().unwrap_or_default();
                let last = messages.iter().rev().find(|m| role(m) == "assistant");
                if let Some(text) = last.map(message_text).filter(|t| !t.is_empty()) {
                    self.final_text = Some(text);
                }
            }
            "agent_settled" => self.settled = true,
            _ => return Effect::None,
        }
        Effect::Changed
    }

    fn end_message(&mut self, message: &Value) {
        self.text = message_text(message);
        if !self.text.is_empty() {
            self.final_text = Some(self.text.clone());
        }
        let stop = message["stopReason"].as_str().unwrap_or("");
        if stop == "error" || stop == "aborted" {
            let reason = message["errorMessage"].as_str().unwrap_or(stop);
            self.error = Some(reason.to_owned());
        } else {
            self.error = None;
        }
        let usage = &message["usage"];
        self.usage.input += usage["input"].as_u64().unwrap_or(0);
        self.usage.output += usage["output"].as_u64().unwrap_or(0);
        self.usage.cost += usage["cost"]["total"].as_f64().unwrap_or(0.0);
        if let (Some(provider), Some(model)) =
            (message["provider"].as_str(), message["model"].as_str())
        {
            self.model = Some(format!("{provider}/{model}"));
        }
    }

    fn end_tool(&mut self, event: &Value) -> Effect {
        let id = str_of(event, "toolCallId");
        let is_error = event["isError"].as_bool().unwrap_or(false);
        if let Some(call) = self.tools.iter_mut().rev().find(|call| call.id == id) {
            call.done = true;
            call.is_error = is_error;
        }
        let details = &event["result"]["details"];
        if event["toolName"] == "todo" && !is_error && details.is_object() {
            return Effect::Todo(details.clone());
        }
        Effect::Changed
    }

    /// The answer of the run: the last assistant text.
    pub fn output(&self) -> String {
        self.final_text.clone().unwrap_or_else(|| self.text.clone())
    }
}

fn role(message: &Value) -> &str {
    message["role"].as_str().unwrap_or("")
}

fn str_of(event: &Value, key: &str) -> String {
    event[key].as_str().unwrap_or("").to_owned()
}

pub fn message_text(message: &Value) -> String {
    match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// One short line for tool arguments: the path, command, or pattern when present.
pub fn summarize(args: &Value) -> String {
    let keys = [
        "path",
        "file_path",
        "command",
        "pattern",
        "query",
        "url",
        "action",
    ];
    let text = keys
        .iter()
        .find_map(|key| args.get(*key).and_then(Value::as_str))
        .map(str::to_owned)
        .unwrap_or_else(|| match args {
            Value::Object(map) if map.is_empty() => String::new(),
            Value::Null => String::new(),
            other => other.to_string(),
        });
    let line = text.lines().next().unwrap_or("").to_owned();
    if line.chars().count() > 80 {
        let cut: String = line.chars().take(79).collect();
        format!("{cut}\u{2026}")
    } else {
        line
    }
}
