//! Inbound `rpiv:ask-user:answer` payloads turned into questionnaire results.

use std::collections::{BTreeMap, HashSet};

use serde_json::{Map, Value};

use crate::result::{AnswerKind, QuestionAnswer, QuestionnaireResult};
use crate::schema::Question;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectCode {
    NotFound,
    InvalidPayload,
    InvalidAnswer,
    UnsupportedHost,
}

impl RejectCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::InvalidPayload => "invalid_payload",
            Self::InvalidAnswer => "invalid_answer",
            Self::UnsupportedHost => "unsupported_host",
        }
    }
}

pub type Rejection = (RejectCode, String);

fn payload(reason: impl Into<String>) -> Rejection {
    (RejectCode::InvalidPayload, reason.into())
}

fn invalid(reason: impl Into<String>) -> Rejection {
    (RejectCode::InvalidAnswer, reason.into())
}

/// Trimmed; `None` when blank.
fn note_of(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

fn optional_str<'a>(
    map: &'a Map<String, Value>,
    key: &str,
    err: &str,
) -> Result<Option<&'a str>, Rejection> {
    match map.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(payload(err)),
    }
}

fn integer(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        value
            .as_f64()
            .filter(|f| f.fract() == 0.0)
            .map(|f| f as i64)
    })
}

/// All or nothing: every question answered once, indexes in range, no blank text.
pub fn resolve(
    questions: &[Question],
    payload_map: &Map<String, Value>,
) -> Result<QuestionnaireResult, Rejection> {
    let cancel = match payload_map.get("cancel") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(payload("cancel must be a boolean")),
    };
    let global = optional_str(payload_map, "globalNote", "globalNote must be a string")?;
    let global_note = note_of(global);
    let answers = payload_map.get("answers");
    if cancel {
        let empty = answers.is_none() || matches!(answers, Some(Value::Array(a)) if a.is_empty());
        if !empty {
            return Err(payload("cancel takes no answers"));
        }
        return Ok(QuestionnaireResult {
            cancelled: true,
            global_note,
            ..QuestionnaireResult::default()
        });
    }
    let Some(Value::Array(answers)) = answers else {
        return Err(payload("answers must be an array"));
    };
    let mut by_question = BTreeMap::new();
    for entry in answers {
        let Value::Object(entry) = entry else {
            return Err(payload("each answer must be an object"));
        };
        let index = entry
            .get("questionIndex")
            .and_then(integer)
            .ok_or_else(|| payload("questionIndex must be an integer"))?;
        if index < 0 || index as usize >= questions.len() {
            let last = questions.len() as i64 - 1;
            return Err(invalid(format!(
                "questionIndex {index} out of range (0..{last})"
            )));
        }
        if by_question.insert(index as usize, entry).is_some() {
            return Err(invalid(format!("question {index} answered twice")));
        }
    }
    let mut resolved = Vec::new();
    for (i, question) in questions.iter().enumerate() {
        let Some(entry) = by_question.get(&i) else {
            return Err(invalid(format!(
                "no answer for question {i} (\"{}\")",
                question.header
            )));
        };
        resolved.push(resolve_entry(question, i, entry)?);
    }
    Ok(QuestionnaireResult {
        answers: resolved,
        global_note,
        ..QuestionnaireResult::default()
    })
}

fn resolve_entry(
    q: &Question,
    i: usize,
    entry: &Map<String, Value>,
) -> Result<QuestionAnswer, Rejection> {
    let picked: Vec<i64> = match entry.get("optionIndexes") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(integer)
            .collect::<Option<Vec<i64>>>()
            .ok_or_else(|| {
                payload(format!(
                    "question {i}: optionIndexes must be an array of integers"
                ))
            })?,
        Some(_) => {
            return Err(payload(format!(
                "question {i}: optionIndexes must be an array of integers"
            )));
        }
    };
    let text = optional_str(
        entry,
        "text",
        &format!("question {i}: text must be a string"),
    )?;
    let notes = optional_str(
        entry,
        "notes",
        &format!("question {i}: notes must be a string"),
    )?;
    let bad = |reason: String| invalid(format!("question {i}: {reason}"));
    let count = q.options.len() as i64;
    if let Some(out) = picked.iter().find(|&&n| n < 0 || n >= count) {
        return Err(bad(format!(
            "option index {out} out of range (0..{})",
            count - 1
        )));
    }
    if picked.iter().collect::<HashSet<_>>().len() != picked.len() {
        return Err(bad("option index repeated".into()));
    }
    if text.is_some_and(|t| t.trim().is_empty()) {
        return Err(bad("text is blank".into()));
    }
    if picked.is_empty() && text.is_none() {
        return Err(bad("needs optionIndexes or text".into()));
    }
    let text = text.map(str::to_owned);
    let mut answer = if q.is_multi() {
        let mut answer = QuestionAnswer::new(i, &q.question, AnswerKind::Multi, text);
        let selected = q
            .options
            .iter()
            .enumerate()
            .filter(|(n, _)| picked.contains(&(*n as i64)))
            .map(|(_, o)| o.label.clone())
            .collect();
        answer.selected = Some(selected);
        answer
    } else if !picked.is_empty() && text.is_some() {
        return Err(bad(
            "single-select takes optionIndexes or text, not both".into()
        ));
    } else if picked.len() > 1 {
        return Err(bad(format!(
            "single-select takes one option, got {}",
            picked.len()
        )));
    } else if text.is_some() {
        QuestionAnswer::new(i, &q.question, AnswerKind::Custom, text)
    } else {
        let option = &q.options[picked[0] as usize];
        let mut answer = QuestionAnswer::new(
            i,
            &q.question,
            AnswerKind::Option,
            Some(option.label.clone()),
        );
        answer.preview = option.preview().map(str::to_owned);
        answer
    };
    answer.notes = note_of(notes);
    Ok(answer)
}
