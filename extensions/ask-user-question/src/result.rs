//! Answers, the questionnaire result, and the text envelope the model reads.

use pi_ext_sdk::ToolResult;
use serde::{Deserialize, Serialize};

use crate::schema::{QuestionParams, QuestionnaireError};

pub const DECLINE_MESSAGE: &str = "User declined to answer questions";
pub const ENVELOPE_PREFIX: &str = "User has answered your questions:";
pub const ENVELOPE_SUFFIX: &str = "You can now continue with the user's answers in mind.";
pub const NO_INPUT_PLACEHOLDER: &str = "(no input)";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnswerKind {
    Option,
    Custom,
    Multi,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionAnswer {
    pub question_index: usize,
    pub question: String,
    pub kind: AnswerKind,
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

impl QuestionAnswer {
    pub fn new(index: usize, question: &str, kind: AnswerKind, answer: Option<String>) -> Self {
        Self {
            question_index: index,
            question: question.to_owned(),
            kind,
            answer,
            selected: None,
            notes: None,
            preview: None,
        }
    }

    /// The answer as one string: labels and custom text, or `(no input)`.
    pub fn scalar(&self) -> String {
        let text = self.answer.as_deref().filter(|a| !a.is_empty());
        match self.kind {
            AnswerKind::Multi => {
                let mut values = self.selected.clone().unwrap_or_default();
                values.extend(text.map(str::to_owned));
                if values.is_empty() {
                    NO_INPUT_PLACEHOLDER.to_owned()
                } else {
                    values.join(", ")
                }
            }
            AnswerKind::Custom => text.unwrap_or(NO_INPUT_PLACEHOLDER).to_owned(),
            AnswerKind::Option => self
                .answer
                .clone()
                .unwrap_or_else(|| NO_INPUT_PLACEHOLDER.to_owned()),
        }
    }

    fn segment(&self) -> String {
        let mut parts = vec![format!("\"{}\"=\"{}\"", self.question, self.scalar())];
        if let Some(preview) = self.preview.as_deref().filter(|p| !p.is_empty()) {
            parts.push(format!("selected preview: {preview}"));
        }
        if let Some(notes) = self.notes.as_deref().filter(|n| !n.is_empty()) {
            parts.push(format!("user notes: {notes}"));
        }
        format!("{}.", parts.join(". "))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionnaireResult {
    pub answers: Vec<QuestionAnswer>,
    pub cancelled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unanswered: Option<Vec<usize>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<QuestionnaireError>,
}

impl QuestionnaireResult {
    pub fn cancelled() -> Self {
        Self {
            cancelled: true,
            ..Self::default()
        }
    }

    pub fn failed(error: QuestionnaireError) -> Self {
        Self {
            cancelled: true,
            error: Some(error),
            ..Self::default()
        }
    }

    fn global_note(&self) -> Option<&str> {
        self.global_note.as_deref().filter(|n| !n.is_empty())
    }
}

pub fn tool_result(text: impl Into<String>, details: &QuestionnaireResult) -> ToolResult {
    ToolResult::text(text).details(details)
}

pub fn error_result(message: impl Into<String>, error: QuestionnaireError) -> ToolResult {
    tool_result(message, &QuestionnaireResult::failed(error))
}

/// Maps a dialog result to the tool result. Cancelled or empty results read as the decline.
pub fn build_response(result: Option<QuestionnaireResult>, params: &QuestionParams) -> ToolResult {
    let result = match result {
        Some(result) if !result.cancelled => result,
        other => {
            let other = other.unwrap_or_default();
            let details = QuestionnaireResult {
                answers: other.answers.clone(),
                cancelled: true,
                global_note: other.global_note().map(str::to_owned),
                ..QuestionnaireResult::default()
            };
            return tool_result(DECLINE_MESSAGE, &details);
        }
    };
    let mut segments = Vec::new();
    let mut unanswered = Vec::new();
    for index in 0..params.questions.len() {
        match result.answers.iter().find(|a| a.question_index == index) {
            Some(answer) => segments.push(answer.segment()),
            None => unanswered.push(index),
        }
    }
    let global_note = result.global_note().map(str::to_owned);
    if segments.is_empty() && global_note.is_none() {
        let details = QuestionnaireResult {
            answers: result.answers,
            cancelled: true,
            ..QuestionnaireResult::default()
        };
        return tool_result(DECLINE_MESSAGE, &details);
    }
    if !unanswered.is_empty() {
        let names: Vec<String> = unanswered
            .iter()
            .map(|&i| format!("\"{}\"", params.questions[i].question))
            .collect();
        segments.push(format!("unanswered: {}.", names.join(", ")));
    }
    if let Some(note) = &global_note {
        segments.push(format!("global note: {note}."));
    }
    let mut details = result;
    if !unanswered.is_empty() {
        details.unanswered = Some(unanswered);
    }
    let text = format!("{ENVELOPE_PREFIX} {} {ENVELOPE_SUFFIX}", segments.join(" "));
    tool_result(text, &details)
}
