use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A tool definition as sent in `initialize` and `register.tool`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub description: String,
    pub parameters: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_snippet: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub prompt_guidelines: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<String>,
}

impl ToolSpec {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            label: None,
            description: description.into(),
            parameters: json!({ "type": "object", "properties": {} }),
            prompt_snippet: None,
            prompt_guidelines: Vec::new(),
            execution_mode: None,
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// JSON Schema object for the tool arguments.
    pub fn parameters(mut self, schema: Value) -> Self {
        self.parameters = schema;
        self
    }

    pub fn prompt_snippet(mut self, snippet: impl Into<String>) -> Self {
        self.prompt_snippet = Some(snippet.into());
        self
    }

    pub fn guideline(mut self, guideline: impl Into<String>) -> Self {
        self.prompt_guidelines.push(guideline.into());
        self
    }

    pub fn execution_mode(mut self, mode: impl Into<String>) -> Self {
        self.execution_mode = Some(mode.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandSpec {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub completions: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShortcutSpec {
    pub shortcut: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FlagType {
    Boolean,
    String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FlagSpec {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub kind: FlagType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

impl FlagSpec {
    pub fn boolean(name: impl Into<String>) -> Self {
        Self::of(name, FlagType::Boolean)
    }

    pub fn string(name: impl Into<String>) -> Self {
        Self::of(name, FlagType::String)
    }

    fn of(name: impl Into<String>, kind: FlagType) -> Self {
        Self {
            name: name.into(),
            description: None,
            kind,
            default: None,
        }
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn default(mut self, value: impl Into<Value>) -> Self {
        self.default = Some(value.into());
        self
    }
}

/// One entry of a `command.complete` or `autocomplete.suggest` answer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompletionItem {
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl CompletionItem {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: None,
            description: None,
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// Params of `autocomplete.suggest`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AutocompleteRequest {
    pub lines: Vec<String>,
    pub cursor_line: usize,
    pub cursor_col: usize,
    #[serde(default)]
    pub force: bool,
}

impl AutocompleteRequest {
    /// The text of the cursor line before the cursor.
    pub fn before_cursor(&self) -> &str {
        let line = self.lines.get(self.cursor_line).map_or("", String::as_str);
        let end = line
            .char_indices()
            .nth(self.cursor_col)
            .map_or(line.len(), |(i, _)| i);
        &line[..end]
    }
}

/// Answer of `autocomplete.suggest`; `prefix` is the text the items replace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Suggestions {
    pub items: Vec<CompletionItem>,
    pub prefix: String,
    /// pi adds its own suggestions after `items` when they replace the same `prefix`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub merge: bool,
}

impl Suggestions {
    pub fn new(items: Vec<CompletionItem>, prefix: impl Into<String>) -> Self {
        Self {
            items,
            prefix: prefix.into(),
            merge: false,
        }
    }

    /// Keep pi's own suggestions after these items.
    pub fn merged(mut self) -> Self {
        self.merge = true;
        self
    }
}

#[cfg(test)]
mod suggestion_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_is_sent_only_when_set() {
        let plain = Suggestions::new(vec![CompletionItem::new("@a")], "@");
        assert_eq!(serde_json::to_value(&plain).unwrap().get("merge"), None);
        let merged = serde_json::to_value(plain.merged()).unwrap();
        assert_eq!(merged["merge"], json!(true));
        assert_eq!(merged["prefix"], json!("@"));
    }
}

/// Params of `virtualModel.route`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RouteRequest {
    pub provider: String,
    pub id: String,
    #[serde(default)]
    pub request: Value,
}
