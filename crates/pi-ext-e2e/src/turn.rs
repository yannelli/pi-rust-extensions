use serde_json::{Value, json};

/// One scripted assistant turn the mock provider streams back.
#[derive(Clone, Debug, PartialEq)]
pub enum Turn {
    Text(String),
    /// `(name, arguments)` of each tool call, in order.
    ToolCalls(Vec<(String, Value)>),
}

impl Turn {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    pub fn tool_call(name: impl Into<String>, arguments: Value) -> Self {
        Self::ToolCalls(vec![(name.into(), arguments)])
    }

    pub fn tool_calls<N: Into<String>>(calls: impl IntoIterator<Item = (N, Value)>) -> Self {
        Self::ToolCalls(
            calls
                .into_iter()
                .map(|(name, args)| (name.into(), args))
                .collect(),
        )
    }

    /// The chat-completions stream chunks of this turn; `index` numbers the tool call ids.
    pub(crate) fn chunks(&self, index: usize) -> Vec<Value> {
        let choice = |delta: Value, finish: Option<&str>| json!({"choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
        match self {
            Self::Text(text) => vec![
                choice(json!({"content": text}), None),
                choice(json!({}), Some("stop")),
            ],
            Self::ToolCalls(calls) => {
                let tool_calls: Vec<Value> = calls
                    .iter()
                    .enumerate()
                    .map(|(position, (name, args))| {
                        json!({"index": position, "id": format!("call_{index}_{position}"), "type": "function",
                            "function": {"name": name, "arguments": args.to_string()}})
                    })
                    .collect();
                vec![
                    choice(json!({"tool_calls": tool_calls}), None),
                    choice(json!({}), Some("tool_calls")),
                ]
            }
        }
    }
}
