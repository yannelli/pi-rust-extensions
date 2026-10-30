use serde_json::{Map, Value};

use crate::backends::Limits;
use crate::errors::{TsError, TsResult};
use crate::schema::{MAX_QUESTIONS, normalize};

pub const MAX_INPUT_BYTES: usize = 65_536;

const USAGE: &str = "Expected { state, questions: { <id>: { type: \"choice\", instructions, criteria: { label: description|null } } | { type: \"score\", instructions, criteria: [level0, level1, ...] } | { type: \"noul\", instructions } } }; 1–32 questions, Choice 1–64 options, Score 2–32 levels.";
const ENTRY: &str = "Expected string, array, object or null";

fn chars(text: &str) -> usize {
    text.encode_utf16().count()
}

fn is_content(value: &Value) -> bool {
    matches!(value, Value::String(_) | Value::Array(_) | Value::Object(_))
}

fn is_entry(value: &Value) -> bool {
    value.is_null() || is_content(value)
}

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[derive(Default)]
struct Errors(Vec<String>);

impl Errors {
    fn push(&mut self, path: &str, message: &str) {
        if self.0.len() < 3 {
            self.0.push(format!("{}: {message}", clip(path, 120)));
        }
    }
}

fn check_noul(errors: &mut Errors, at: &str, criteria: Option<&Value>) {
    match criteria {
        None | Some(Value::Null) => {}
        Some(Value::Object(criteria)) => {
            for (key, value) in criteria {
                if !matches!(key.as_str(), "true" | "false") {
                    errors.push(&format!("{at}.{key}"), "Unexpected property");
                } else if !is_entry(value) {
                    errors.push(&format!("{at}.{key}"), ENTRY);
                }
            }
        }
        Some(_) => errors.push(at, "Expected null or { true, false }"),
    }
}

fn check_choice(errors: &mut Errors, at: &str, criteria: Option<&Value>) {
    let Some(Value::Object(criteria)) = criteria else {
        return errors.push(at, "Expected object of label to description");
    };
    if criteria.is_empty() || criteria.len() > 64 {
        errors.push(at, "Expected 1 to 64 options");
    }
    for (label, value) in criteria {
        if !(1..=200).contains(&chars(label)) {
            errors.push(at, "Expected option labels of 1 to 200 characters");
        } else if !is_entry(value) {
            errors.push(&format!("{at}.{label}"), ENTRY);
        }
    }
}

fn check_score(errors: &mut Errors, at: &str, criteria: Option<&Value>) {
    let Some(Value::Array(levels)) = criteria else {
        return errors.push(at, "Expected array of levels");
    };
    if !(2..=32).contains(&levels.len()) {
        errors.push(at, "Expected 2 to 32 levels");
    }
    if !levels.iter().all(is_entry) {
        errors.push(at, "Expected levels of string, array, object or null");
    }
}

fn check_question(errors: &mut Errors, path: &str, question: &Value) {
    let Some(item) = question.as_object() else {
        return errors.push(path, "Expected object");
    };
    for key in item.keys() {
        if !matches!(key.as_str(), "type" | "instructions" | "criteria") {
            errors.push(&format!("{path}.{key}"), "Unexpected property");
        }
    }
    if item.get("instructions").is_some_and(|i| !is_entry(i)) {
        errors.push(&format!("{path}.instructions"), ENTRY);
    }
    let at = format!("{path}.criteria");
    let criteria = item.get("criteria");
    match item.get("type").and_then(Value::as_str) {
        Some("noul") => check_noul(errors, &at, criteria),
        Some("choice") => check_choice(errors, &at, criteria),
        Some("score") => check_score(errors, &at, criteria),
        _ => errors.push(
            &format!("{path}.type"),
            "Expected \"noul\", \"choice\" or \"score\"",
        ),
    }
}

/// A `box` question or a `null` state get their own message; the schema reports others.
fn refuse_unsupported(request: &Map<String, Value>) -> TsResult<()> {
    if request.get("state").is_some_and(Value::is_null) {
        return Err(TsError::validation(format!(
            "state must not be null: send what to judge as text, an object, or an array. {USAGE}"
        )));
    }
    let Some(Value::Object(questions)) = request.get("questions") else {
        return Ok(());
    };
    for (id, question) in questions {
        let Some(kind) = question.get("type").and_then(Value::as_str) else {
            continue;
        };
        if !matches!(kind, "noul" | "choice" | "score") {
            return Err(TsError::validation(format!(
                "Invalid evaluation request at questions.{}.type: {} is not a question type this package sends. {USAGE}",
                clip(id, 100),
                Value::String(clip(kind, 40))
            )));
        }
    }
    Ok(())
}

/// Strict schema check. Messages name paths only, never submitted values.
pub fn parse(value: &Value) -> TsResult<()> {
    let mut errors = Errors::default();
    let Some(request) = value.as_object() else {
        errors.push("request", "Expected object");
        return finish(errors);
    };
    refuse_unsupported(request)?;
    for key in request.keys() {
        if !matches!(key.as_str(), "state" | "questions" | "model") {
            errors.push(key, "Unexpected property");
        }
    }
    match request.get("state") {
        None => errors.push("request", "Expected required property state"),
        Some(state) if !is_content(state) => {
            errors.push("state", "Expected string, array or object")
        }
        Some(_) => {}
    }
    match request.get("questions") {
        None => errors.push("request", "Expected required property questions"),
        Some(Value::Object(questions)) => {
            if questions.is_empty() || questions.len() > MAX_QUESTIONS {
                errors.push("questions", "Expected 1 to 32 questions");
            }
            for (id, question) in questions {
                if !(1..=100).contains(&chars(id)) {
                    errors.push("questions", "Expected question ids of 1 to 100 characters");
                }
                check_question(&mut errors, &format!("questions.{id}"), question);
            }
        }
        Some(_) => errors.push("questions", "Expected object"),
    }
    match request.get("model") {
        None => {}
        Some(Value::String(m)) if (1..=100).contains(&chars(m)) => {}
        Some(_) => errors.push("model", "Expected string of 1 to 100 characters"),
    }
    finish(errors)
}

fn finish(errors: Errors) -> TsResult<()> {
    if errors.0.is_empty() {
        return Ok(());
    }
    Err(TsError::validation(format!(
        "Invalid evaluation request at {}. {USAGE}",
        errors.0.join("; ")
    )))
}

/// Counts only: a request the schema accepts and the backend refuses.
pub fn check_limits(request: &Value, limits: &Limits) -> TsResult<()> {
    let refuse = |detail: String| {
        TsError::validation(format!("Evaluation exceeds the backend's limits: {detail}"))
    };
    let empty = Map::new();
    let questions = request["questions"].as_object().unwrap_or(&empty);
    if questions.len() > limits.max_questions {
        return Err(refuse(format!(
            "{} questions, at most {} per request. evaluateAll splits a request to this limit.",
            questions.len(),
            limits.max_questions
        )));
    }
    for (id, question) in questions {
        let where_ = format!("questions.{}", clip(id, 100));
        let criteria = &question["criteria"];
        match question["type"].as_str() {
            Some("choice") => {
                let options = criteria.as_object().map_or(0, Map::len);
                if options < limits.min_choice_options || options > limits.max_choice_options {
                    return Err(refuse(format!(
                        "{where_} has {options} choice options; {}–{} allowed.",
                        limits.min_choice_options, limits.max_choice_options
                    )));
                }
            }
            Some("score") => {
                let levels = criteria.as_array().map_or(0, Vec::len);
                if levels > limits.max_score_levels {
                    return Err(refuse(format!(
                        "{where_} has {levels} score levels; 2–{} allowed.",
                        limits.max_score_levels
                    )));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn check_bytes(text: &str, max: usize) -> TsResult<()> {
    if text.len() > max {
        return Err(TsError::validation(format!(
            "Evaluation exceeds the {max}-byte input limit."
        )));
    }
    Ok(())
}

/// Normalize aliases, validate the schema, check backend limits, then the byte budget.
pub fn prepare(value: Value, limits: Option<&Limits>) -> TsResult<Value> {
    let request = normalize(value);
    parse(&request)?;
    if let Some(limits) = limits {
        check_limits(&request, limits)?;
    }
    check_bytes(&request.to_string(), MAX_INPUT_BYTES)?;
    Ok(request)
}

#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
