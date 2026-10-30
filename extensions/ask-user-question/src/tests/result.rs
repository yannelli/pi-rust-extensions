use serde_json::json;

use super::{params, question};
use crate::result::{AnswerKind, QuestionAnswer, QuestionnaireResult, build_response};

fn text(result: &pi_ext_sdk::ToolResult) -> String {
    result.content[0]["text"].as_str().unwrap().to_owned()
}

fn two() -> crate::schema::QuestionParams {
    params(vec![
        question("Which db?", "DB", &["Postgres", "SQLite"], false),
        question("Which features?", "F", &["Auth", "Billing"], true),
    ])
}

#[test]
fn envelope_for_full_answers() {
    let mut first =
        QuestionAnswer::new(0, "Which db?", AnswerKind::Option, Some("Postgres".into()));
    first.preview = Some("PG".into());
    first.notes = Some("fast".into());
    let mut second =
        QuestionAnswer::new(1, "Which features?", AnswerKind::Multi, Some("SSO".into()));
    second.selected = Some(vec!["Auth".into()]);
    let result = QuestionnaireResult {
        answers: vec![first, second],
        global_note: Some("ship it".into()),
        ..QuestionnaireResult::default()
    };
    let tool = build_response(Some(result), &two());
    assert_eq!(
        text(&tool),
        "User has answered your questions: \"Which db?\"=\"Postgres\". selected preview: PG. user notes: fast. \"Which features?\"=\"Auth, SSO\". global note: ship it. You can now continue with the user's answers in mind."
    );
    let details = tool.details.unwrap();
    assert_eq!(details["cancelled"], false);
    assert_eq!(details["answers"][0]["questionIndex"], 0);
    assert_eq!(details["answers"][1]["kind"], "multi");
    assert!(details.get("unanswered").is_none());
}

#[test]
fn partial_submit_names_unanswered() {
    let answer = QuestionAnswer::new(1, "Which features?", AnswerKind::Multi, None);
    let result = QuestionnaireResult {
        answers: vec![answer],
        ..QuestionnaireResult::default()
    };
    let tool = build_response(Some(result), &two());
    assert_eq!(
        text(&tool),
        "User has answered your questions: \"Which features?\"=\"(no input)\". unanswered: \"Which db?\". You can now continue with the user's answers in mind."
    );
    assert_eq!(tool.details.unwrap()["unanswered"], json!([0]));
}

#[test]
fn decline_for_cancel_and_empty() {
    let cancelled = QuestionnaireResult {
        cancelled: true,
        global_note: Some("later".into()),
        ..QuestionnaireResult::default()
    };
    let tool = build_response(Some(cancelled), &two());
    assert_eq!(text(&tool), "User declined to answer questions");
    assert_eq!(
        tool.details.unwrap(),
        json!({ "answers": [], "cancelled": true, "globalNote": "later" })
    );
    let empty = build_response(Some(QuestionnaireResult::default()), &two());
    assert_eq!(text(&empty), "User declined to answer questions");
    assert_eq!(empty.details.unwrap()["cancelled"], true);
    let note_only = QuestionnaireResult {
        global_note: Some("n".into()),
        ..QuestionnaireResult::default()
    };
    let tool = build_response(Some(note_only), &two());
    assert!(
        text(&tool).starts_with("User has answered your questions: unanswered:"),
        "{}",
        text(&tool)
    );
}
