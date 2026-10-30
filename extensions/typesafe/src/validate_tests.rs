use serde_json::json;

use super::*;
use crate::backends::backend;
use crate::sample;

fn err(value: Value) -> String {
    prepare(value, None).unwrap_err().message
}

#[test]
fn accepts_sample_and_aliases() {
    assert!(prepare(sample(), None).is_ok());
    let alias = json!({ "state": { "a": 1 }, "questions": {
        "c": { "type": "choice", "options": ["x"] }, "n": { "type": "noul", "criteria": "yes" }
    }, "model": "jev-latest" });
    assert!(prepare(alias, None).is_ok());
}

#[test]
fn refuses_bad_shapes() {
    assert!(err(json!({ "state": null, "questions": {} })).starts_with("state must not be null"));
    let boxed = json!({ "state": "s", "questions": { "b": { "type": "box" } } });
    assert!(err(boxed).contains("questions.b.type: \"box\" is not a question type"));
    let extra = json!({ "state": "s", "questions": { "q": { "type": "noul" } }, "x": 1 });
    assert!(err(extra).contains("x: Unexpected property"));
    assert!(
        err(json!({ "state": 1, "questions": { "q": { "type": "noul" } } }))
            .contains("state: Expected")
    );
    assert!(err(json!({ "state": "s", "questions": {} })).contains("Expected 1 to 32 questions"));
    let score =
        json!({ "state": "s", "questions": { "q": { "type": "score", "criteria": ["one"] } } });
    assert!(err(score).contains("questions.q.criteria: Expected 2 to 32 levels"));
    let noul = json!({ "state": "s", "questions": { "q": { "type": "noul", "criteria": { "maybe": 1 } } } });
    assert!(err(noul).contains("questions.q.criteria.maybe: Unexpected property"));
    let long = "m".repeat(101);
    assert!(
        err(json!({ "state": "s", "questions": { "q": { "type": "noul" } }, "model": long }))
            .contains("model:")
    );
    assert!(err(json!("text")).contains("request: Expected object"));
}

#[test]
fn reports_at_most_three_errors_without_values() {
    let request = json!({ "state": "secret-value", "a": 1, "b": 2, "c": 3, "d": 4, "questions": { "q": { "type": "noul" } } });
    let message = err(request);
    assert_eq!(message.matches("Unexpected property").count(), 3);
    assert!(!message.contains("secret-value"));
}

#[test]
fn enforces_backend_limits_and_bytes() {
    let subq = backend("subq").unwrap();
    let one = json!({ "state": "s", "questions": { "c": { "type": "choice", "criteria": { "only": null } } } });
    let message = prepare(one.clone(), Some(&subq.limits))
        .unwrap_err()
        .message;
    assert!(message.contains("questions.c has 1 choice options; 2–64 allowed."));
    assert!(prepare(one, Some(&backend("typesafe").unwrap().limits)).is_ok());
    let questions: Map<String, Value> = (0..17)
        .map(|i| (format!("q{i}"), json!({ "type": "noul" })))
        .collect();
    let many = json!({ "state": "s", "questions": questions });
    assert!(
        prepare(many, Some(&subq.limits))
            .unwrap_err()
            .message
            .contains("17 questions, at most 16")
    );
    let big =
        json!({ "state": "x".repeat(MAX_INPUT_BYTES), "questions": { "q": { "type": "noul" } } });
    assert_eq!(err(big), "Evaluation exceeds the 65536-byte input limit.");
}
