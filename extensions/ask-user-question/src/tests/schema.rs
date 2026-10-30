use serde_json::json;

use super::{params, question};
use crate::schema::{QuestionnaireError as E, normalize, prepare_arguments, validate};

fn code(questions: Vec<serde_json::Value>) -> Option<E> {
    validate(&params(questions)).err().map(|(code, _)| code)
}

#[test]
fn validation_codes() {
    let q = |text: &str, labels: &[&str]| question(text, "H", labels, false);
    assert_eq!(code(vec![]), Some(E::NoQuestions));
    let five = (0..5).map(|i| q(&format!("Q{i}?"), &["a", "b"])).collect();
    assert_eq!(code(five), Some(E::TooManyQuestions));
    assert_eq!(
        code(vec![q("Same?", &["a", "b"]), q("Same?", &["c", "d"])]),
        Some(E::DuplicateQuestion)
    );
    assert_eq!(code(vec![q("One?", &["a"])]), Some(E::EmptyOptions));
    assert_eq!(code(vec![q("R?", &["Other", "b"])]), Some(E::ReservedLabel));
    assert_eq!(
        code(vec![q("R?", &["Next", "Next"])]),
        Some(E::ReservedLabel)
    );
    assert_eq!(
        code(vec![q("R?", &["Type something.", "b"])]),
        Some(E::ReservedLabel)
    );
    assert_eq!(
        code(vec![q("D?", &["a", "a"])]),
        Some(E::DuplicateOptionLabel)
    );
    assert_eq!(code(vec![q("Ok?", &["a", "b"])]), None);
}

#[test]
fn validation_messages_match_upstream() {
    let err = validate(&params(vec![question("R?", "H", &["Other", "b"], false)])).unwrap_err();
    assert_eq!(
        err.1,
        "Error: Option label is reserved (Other, Type something., Next)"
    );
    assert_eq!(
        serde_json::to_value(E::DuplicateOptionLabel).unwrap(),
        json!("duplicate_option_label")
    );
}

#[test]
fn normalizes_line_terminators_before_validation() {
    let raw = params(vec![json!({
        "question": "GEMBA\r_LOG?\r\n", "header": "H\r",
        "options": [
            { "label": "Other\r", "description": "a\r\nb" },
            { "label": "x", "description": "y", "preview": "p\r\nq" }
        ]
    })]);
    let typed = normalize(raw);
    let q = &typed.questions[0];
    assert_eq!(q.question, "GEMBA_LOG?\n");
    assert_eq!(q.header, "H");
    assert_eq!(q.options[0].label, "Other");
    assert_eq!(q.options[0].description, "a\nb");
    assert_eq!(q.options[1].preview.as_deref(), Some("p\nq"));
    assert_eq!(validate(&typed).unwrap_err().0, E::ReservedLabel);
}

#[test]
fn decodes_double_encoded_arguments() {
    let option = json!({ "label": "a", "description": "d" }).to_string();
    let inner = json!({ "question": "Q?", "header": "H", "options": [option, { "label": "b", "description": "e" }] });
    let args = json!({ "questions": json!([inner.to_string()]).to_string() });
    let decoded = prepare_arguments(args);
    assert_eq!(decoded["questions"][0]["options"][0]["label"], "a");
    assert_eq!(decoded["questions"][0]["options"][1]["label"], "b");
    let untouched = prepare_arguments(json!({ "questions": "not json" }));
    assert_eq!(untouched["questions"], "not json");
}
