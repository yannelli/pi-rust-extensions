mod channel;
mod config;
mod fallback;
mod keys;
mod render;
mod result;
mod schema;

use serde_json::{Value, json};

use crate::schema::{Question, QuestionParams};

pub fn question(text: &str, header: &str, labels: &[&str], multi: bool) -> Value {
    let options: Vec<Value> = labels
        .iter()
        .map(|l| json!({ "label": l, "description": format!("{l} description") }))
        .collect();
    json!({ "question": text, "header": header, "options": options, "multiSelect": multi })
}

pub fn params(questions: Vec<Value>) -> QuestionParams {
    serde_json::from_value(json!({ "questions": questions })).unwrap()
}

pub fn questions(questions: Vec<Value>) -> Vec<Question> {
    params(questions).questions
}

pub fn two_questions() -> Vec<Question> {
    questions(vec![
        question("Which db?", "DB", &["Postgres", "SQLite"], false),
        question(
            "Which features?",
            "Features",
            &["Auth", "Billing", "Search"],
            true,
        ),
    ])
}
