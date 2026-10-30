use std::sync::Arc;

use super::*;
use crate::backends::from_values;
use crate::mock_api::{MockApi, Reply};
use crate::testutil::TempDir;

const KEY: &str = "ts_test_0123456789abcdef";

fn request() -> Value {
    json!({ "state": "charged twice", "questions": {
        "c": { "type": "choice", "criteria": { "billing": null, "other": null } },
        "n": { "type": "noul", "instructions": "Urgent?" }
    }})
}

fn answer() -> Value {
    json!({ "model": "jev-1.13", "usage": { "input_tokens": 1000, "output_tokens": 3 }, "answers": {
        "c": { "type": "choice", "choice": "billing", "confidence": 0.8, "probabilities": { "billing": 0.8, "other": 0.2 } },
        "n": { "type": "noul", "noul": 0.7 }
    }})
}

fn client(api: &MockApi, dir: &TempDir, backend: &str) -> Client {
    let backend = from_values(Some(backend), Some(&api.url)).unwrap();
    let env = Env::fixed::<&str, &str>([]);
    let clock: Clock = Arc::new(|| "2026-10-29".to_owned());
    Client::new(
        backend,
        Some(KEY.into()),
        None,
        dir.path(),
        &env,
        Caps::default(),
        clock,
    )
    .unwrap()
}

#[tokio::test]
async fn evaluates_and_counts() {
    let api = MockApi::start(vec![Reply::json(200, &answer())]).await;
    let dir = TempDir::new();
    let client = client(&api, &dir, "typesafe");
    let result = client
        .evaluate(request(), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result["answers"]["c"]["choice"], "billing");
    assert!(result["elapsedMs"].is_u64());
    let sent = &api.requests()[0];
    assert_eq!(
        (sent.method.as_str(), sent.path.as_str()),
        ("POST", "/v1/systemone")
    );
    assert_eq!(
        sent.authorization.as_deref(),
        Some(format!("Bearer {KEY}").as_str())
    );
    let body: Value = serde_json::from_str(&sent.body).unwrap();
    assert_eq!(body["model"], "jev-latest");
    let spend = client.spend();
    assert_eq!(spend.session.requests_succeeded, 1);
    assert_eq!(spend.session.input_tokens, 1000);
    assert_eq!(spend.today.totals.requests_started, 1);
    assert!(AuthState::new(dir.path()).read().verified_at.is_some());
}

#[tokio::test]
async fn http_errors_are_safe_and_recorded() {
    let body = json!({ "error": "bad key ts_test_0123456789abcdef" });
    let api = MockApi::start(vec![
        Reply::json(401, &body),
        Reply::json(429, &body).header("retry-after", "7"),
    ])
    .await;
    let dir = TempDir::new();
    let client = client(&api, &dir, "openrouter");
    let error = client
        .evaluate(request(), &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!((error.code, error.status), (Code::Http, Some(401)));
    assert!(error.message.contains("Check OPENROUTER_API_KEY."));
    assert!(!error.message.contains(KEY));
    assert_eq!(api.requests()[0].path, "/api/alpha/decisions");
    assert!(AuthState::new(dir.path()).read().rejected());
    let error = client
        .evaluate(request(), &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(error.message.contains("Retry after 7 seconds."));
    assert_eq!(client.spend().session.requests_failed, 2);
}

#[tokio::test]
async fn malformed_answers_and_limits() {
    let mut wrong = answer();
    wrong["answers"]["n"]["noul"] = json!(3);
    let api = MockApi::start(vec![Reply::json(200, &wrong)]).await;
    let dir = TempDir::new();
    let client = client(&api, &dir, "typesafe");
    let error = client
        .evaluate(request(), &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(
        error.message,
        "TypeSafe returned an unexpected answer or usage format."
    );
    let invalid = client
        .evaluate(json!({ "state": null }), &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(invalid.code, Code::Validation);
    assert_eq!(api.requests().len(), 1);
    for _ in 1..DEFAULT_MAX_REQUESTS {
        let _ = client.evaluate(request(), &CancellationToken::new()).await;
    }
    let budget = client
        .evaluate(request(), &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(budget.code, Code::Budget);
    assert!(budget.message.contains("20 attempts per client instance"));
}

#[tokio::test]
async fn daily_cap_blocks_before_sending() {
    let api = MockApi::start(vec![Reply::json(200, &answer())]).await;
    let dir = TempDir::new();
    let backend = from_values(None, Some(&api.url)).unwrap();
    let env = Env::fixed([("PI_TYPESAFE_MAX_REQUESTS_PER_DAY", "1")]);
    let clock: Clock = Arc::new(|| "2026-10-29".to_owned());
    let client = Client::new(
        backend,
        Some(KEY.into()),
        None,
        dir.path(),
        &env,
        Caps::default(),
        clock,
    )
    .unwrap();
    client
        .evaluate(request(), &CancellationToken::new())
        .await
        .unwrap();
    let error = client
        .evaluate(request(), &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(
        error
            .message
            .starts_with("TypeSafe daily request cap reached (1 of 1 on 2026-10-29)")
    );
    assert_eq!(api.requests().len(), 1);
}

#[tokio::test]
async fn lists_models_and_cancels() {
    let models = json!({ "data": [{ "id": "typesafe/jev-1.13" }, { "id": "" }] });
    let api = MockApi::start(vec![Reply::json(200, &models)]).await;
    let dir = TempDir::new();
    let client = client(&api, &dir, "openrouter");
    let names = client.list_models(&CancellationToken::new()).await.unwrap();
    assert_eq!(names, ["typesafe/jev-1.13"]);
    assert_eq!(api.requests()[0].path, "/api/v1/models");
    assert!(AuthState::new(dir.path()).read().verified_at.is_none());
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = client.evaluate(request(), &cancel).await.unwrap_err();
    assert_eq!(
        error.message,
        "TypeSafe request cancelled before submission."
    );
}

#[test]
fn missing_key_names_login_and_variable() {
    let dir = TempDir::new();
    let env = Env::fixed::<&str, &str>([]);
    let clock: Clock = Arc::new(|| "2026-10-29".to_owned());
    let backend = from_values(Some("liquid"), None).unwrap();
    let error = Client::new(
        backend,
        None,
        None,
        dir.path(),
        &env,
        Caps::default(),
        clock,
    )
    .err()
    .unwrap();
    assert_eq!(
        error.message,
        "No API key. Run /typesafe login liquid in Pi, or set LIQUID_API_KEY in the environment."
    );
}
