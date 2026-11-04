//! Tool parameters: types, JSON Schema, argument decoding, normalization, validation.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const MAX_QUESTIONS: usize = 4;
pub const MIN_OPTIONS: usize = 2;
pub const MAX_OPTIONS: usize = 4;
pub const MAX_HEADER_LENGTH: usize = 16;
pub const MAX_LABEL_LENGTH: usize = 60;

pub const OTHER_LABEL: &str = "Type something.";
pub const NEXT_LABEL: &str = "Next";
pub const RESERVED_LABELS: [&str; 3] = ["Other", OTHER_LABEL, NEXT_LABEL];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

impl QuestionOption {
    pub fn preview(&self) -> Option<&str> {
        self.preview.as_deref().filter(|p| !p.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub question: String,
    pub header: String,
    pub options: Vec<QuestionOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multi_select: Option<bool>,
}

impl Question {
    pub fn is_multi(&self) -> bool {
        self.multi_select == Some(true)
    }

    pub fn has_preview(&self) -> bool {
        self.options.iter().any(|o| o.preview().is_some())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionParams {
    pub questions: Vec<Question>,
}

/// Validation failures and host failures carried in `details.error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionnaireError {
    NoUi,
    NoCustomUi,
    NoQuestions,
    EmptyOptions,
    TooManyQuestions,
    DuplicateQuestion,
    DuplicateOptionLabel,
    ReservedLabel,
    SessionLoadFailed,
    StaleModuleCache,
}

pub const ERROR_NO_QUESTIONS: &str = "Error: At least one question is required";
pub const ERROR_DUPLICATE_QUESTION: &str =
    "Error: Question text must be unique within an invocation";
pub const ERROR_DUPLICATE_OPTION_LABEL: &str =
    "Error: Option labels must be unique within a question";

pub fn error_too_many_questions() -> String {
    format!("Error: At most {MAX_QUESTIONS} questions are allowed per invocation")
}

pub fn error_too_few_options() -> String {
    format!("Error: Each question requires at least {MIN_OPTIONS} options")
}

pub fn error_reserved_label() -> String {
    format!(
        "Error: Option label is reserved ({})",
        RESERVED_LABELS.join(", ")
    )
}

/// Checks every rule except `no_ui`; `reserved_label` wins over `duplicate_option_label`.
pub fn validate(params: &QuestionParams) -> Result<(), (QuestionnaireError, String)> {
    use QuestionnaireError as E;
    if params.questions.is_empty() {
        return Err((E::NoQuestions, ERROR_NO_QUESTIONS.into()));
    }
    if params.questions.len() > MAX_QUESTIONS {
        return Err((E::TooManyQuestions, error_too_many_questions()));
    }
    let mut seen = HashSet::new();
    for q in &params.questions {
        if !seen.insert(q.question.as_str()) {
            return Err((E::DuplicateQuestion, ERROR_DUPLICATE_QUESTION.into()));
        }
    }
    for q in &params.questions {
        if q.options.len() < MIN_OPTIONS {
            return Err((E::EmptyOptions, error_too_few_options()));
        }
        let mut labels = HashSet::new();
        for o in &q.options {
            if RESERVED_LABELS.contains(&o.label.as_str()) {
                return Err((E::ReservedLabel, error_reserved_label()));
            }
            if !labels.insert(o.label.as_str()) {
                return Err((E::DuplicateOptionLabel, ERROR_DUPLICATE_OPTION_LABEL.into()));
            }
        }
    }
    Ok(())
}

/// `\r\n` becomes `\n`; a lone `\r` is removed.
pub fn normalize_text(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "")
}

pub fn normalize(params: QuestionParams) -> QuestionParams {
    let questions = params
        .questions
        .into_iter()
        .map(|q| Question {
            question: normalize_text(&q.question),
            header: normalize_text(&q.header),
            options: q
                .options
                .into_iter()
                .map(|o| QuestionOption {
                    label: normalize_text(&o.label),
                    description: normalize_text(&o.description),
                    preview: o.preview.as_deref().map(normalize_text),
                })
                .collect(),
            multi_select: q.multi_select,
        })
        .collect();
    QuestionParams { questions }
}

/// Decodes `questions`, its items, and their `options` when a model sent them as JSON strings.
pub fn prepare_arguments(args: Value) -> Value {
    let Value::Object(mut map) = args else {
        return args;
    };
    let Some(Value::Array(questions)) = map.get("questions").cloned().map(decode_array) else {
        return Value::Object(map);
    };
    let decoded: Vec<Value> = questions
        .into_iter()
        .map(decode_object)
        .map(|q| match q {
            Value::Object(mut question) => {
                if let Some(Value::Array(options)) =
                    question.get("options").cloned().map(decode_array)
                {
                    let options = options.into_iter().map(decode_object).collect();
                    question.insert("options".into(), Value::Array(options));
                }
                Value::Object(question)
            }
            other => other,
        })
        .collect();
    map.insert("questions".into(), Value::Array(decoded));
    Value::Object(map)
}

fn decode_array(value: Value) -> Value {
    match &value {
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(parsed @ Value::Array(_)) => parsed,
            _ => value,
        },
        _ => value,
    }
}

fn decode_object(value: Value) -> Value {
    match &value {
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(parsed @ Value::Object(_)) => parsed,
            _ => value,
        },
        _ => value,
    }
}

pub fn parameters_schema() -> Value {
    let option = json!({
        "type": "object",
        "properties": {
            "label": { "type": "string", "maxLength": MAX_LABEL_LENGTH, "description": "1-5 words; unique; reserved: Other, Type something., Next" },
            "description": { "type": "string", "description": "Meaning or trade-offs" },
            "preview": { "type": "string", "description": "Markdown artifact comparison; single-select only; omit for simple preferences" }
        },
        "required": ["label", "description"]
    });
    let question = json!({
        "type": "object",
        "properties": {
            "question": { "type": "string", "description": "Unique, specific question ending in ?" },
            "header": { "type": "string", "maxLength": MAX_HEADER_LENGTH, "description": "Short question tag" },
            "options": { "type": "array", "items": option, "minItems": MIN_OPTIONS, "maxItems": MAX_OPTIONS, "description": "Distinct choices" },
            "multiSelect": { "type": "boolean", "default": false, "description": "Allow multiple answers" }
        },
        "required": ["question", "header", "options"]
    });
    let mut properties = Map::new();
    properties.insert(
        "questions".into(),
        json!({ "type": "array", "items": question, "minItems": 1, "maxItems": MAX_QUESTIONS }),
    );
    json!({ "type": "object", "properties": properties, "required": ["questions"] })
}
