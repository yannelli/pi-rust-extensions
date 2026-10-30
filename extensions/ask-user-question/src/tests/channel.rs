use serde_json::{Value, json};

use super::two_questions;
use crate::channel::{AnswerChannel, AnswerHost};

fn code(verdict: &Value) -> &str {
    verdict["code"].as_str().unwrap_or("ok")
}

#[test]
fn verdicts_for_bad_payloads() {
    let channel = AnswerChannel::default();
    let open = channel.open("t1", &two_questions(), AnswerHost::Tui);
    assert_eq!(code(&channel.handle(&json!("x"))), "invalid_payload");
    assert_eq!(
        code(&channel.handle(&json!({ "toolCallId": 3 }))),
        "invalid_payload"
    );
    assert_eq!(
        code(&channel.handle(&json!({ "toolCallId": "t1", "requestId": 1 }))),
        "invalid_payload"
    );
    assert_eq!(
        code(&channel.handle(&json!({ "toolCallId": "nope", "answers": [] }))),
        "not_found"
    );
    let missing = json!({ "toolCallId": "t1", "requestId": "r", "answers": [{ "questionIndex": 0, "optionIndexes": [0] }] });
    let verdict = channel.handle(&missing);
    assert_eq!(code(&verdict), "invalid_answer");
    assert_eq!(verdict["requestId"], "r");
    assert_eq!(verdict["toolCallId"], "t1");
    let both = json!({ "toolCallId": "t1", "answers": [
        { "questionIndex": 0, "optionIndexes": [0], "text": "x" },
        { "questionIndex": 1, "optionIndexes": [0] }
    ] });
    assert_eq!(code(&channel.handle(&both)), "invalid_answer");
    let range = json!({ "toolCallId": "t1", "answers": [
        { "questionIndex": 0, "optionIndexes": [5] }, { "questionIndex": 1, "text": " " }
    ] });
    assert_eq!(code(&channel.handle(&range)), "invalid_answer");
    assert!(open.settled().is_none(), "rejected answers apply nothing");
}

#[test]
fn first_answer_wins() {
    let channel = AnswerChannel::default();
    let open = channel.open("t1", &two_questions(), AnswerHost::Tui);
    let answer = json!({ "toolCallId": "t1", "globalNote": " g ", "answers": [
        { "questionIndex": 0, "optionIndexes": [1], "notes": " why " },
        { "questionIndex": 1, "optionIndexes": [2, 0], "text": "more" }
    ] });
    assert_eq!(
        channel.handle(&answer),
        json!({ "ok": true, "toolCallId": "t1" })
    );
    let result = open.settled().unwrap();
    assert_eq!(result.global_note.as_deref(), Some("g"));
    assert_eq!(result.answers[0].answer.as_deref(), Some("SQLite"));
    assert_eq!(result.answers[0].notes.as_deref(), Some("why"));
    let selected = result.answers[1].selected.clone().unwrap();
    assert_eq!(selected, ["Auth", "Search"]);
    assert_eq!(code(&channel.handle(&answer)), "not_found");
    assert!(!open.settle(Default::default()));
}

#[test]
fn rpc_host_and_cancel() {
    let channel = AnswerChannel::default();
    let open = channel.open("r1", &two_questions(), AnswerHost::Rpc);
    let cancel = json!({ "toolCallId": "r1", "cancel": true });
    assert_eq!(code(&channel.handle(&cancel)), "unsupported_host");
    open.set_host(AnswerHost::Tui);
    let bad = json!({ "toolCallId": "r1", "cancel": true, "answers": [{}] });
    assert_eq!(code(&channel.handle(&bad)), "invalid_payload");
    assert_eq!(code(&channel.handle(&cancel)), "ok");
    assert!(open.settled().unwrap().cancelled);
    channel.close(&open);
    let other = channel.open("r2", &two_questions(), AnswerHost::Tui);
    assert_eq!(
        code(&channel.handle(&json!({ "toolCallId": "r1", "cancel": true }))),
        "not_found"
    );
    drop(other);
}
