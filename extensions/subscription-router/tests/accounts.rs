mod common;

use common::http::{MockHttp, Reply};
use common::now_ms;
use pi_ext_subscription_router::accounts::{Account, AccountStore};
use pi_ext_subscription_router::family::Family;
use pi_ext_subscription_router::oauth::{Credential, anthropic, codex};
use serde_json::{Value, json};

fn credential(refresh: &str, expires: i64) -> Credential {
    Credential {
        access: format!("access-{refresh}"),
        refresh: refresh.into(),
        expires,
        account_id: None,
    }
}

#[tokio::test]
async fn token_errors_do_not_contain_tokens() {
    let secret = "secret-token-value";
    let missing = json!({ "access_token": secret, "refresh_token": "r-secret" });
    let rejected = json!({ "error": "invalid_grant", "error_description": "Refresh token reused", "access_token": secret });
    let mock = MockHttp::start(vec![
        Reply::json(200, missing),
        Reply::json(401, rejected),
        Reply::json(500, json!({ "detail": secret })),
    ])
    .await;
    let client = reqwest::Client::new();
    let url = format!("{}/oauth/token", mock.url);
    let error = codex::refresh(&client, &url, "r1").await.unwrap_err();
    assert_eq!(
        error,
        "OpenAI Codex refresh token response missing fields: expires_in"
    );
    let error = anthropic::refresh(&client, &url, "r1").await.unwrap_err();
    assert_eq!(
        error,
        "Anthropic token request failed (401): invalid_grant: Refresh token reused"
    );
    let error = anthropic::refresh(&client, &url, "r1").await.unwrap_err();
    assert_eq!(error, "Anthropic token request failed (500)");
}

#[tokio::test]
async fn corrupt_accounts_file_is_not_overwritten() {
    let dir = common::temp_dir("corrupt");
    let path = dir.join("accounts.json");
    let corrupt = "{ \"accounts\": ";
    std::fs::write(&path, corrupt).unwrap();
    let store = AccountStore::load(path.clone());
    assert!(store.list().is_empty());
    let account = Account::new(Family::Codex, 2, Some(credential("r", now_ms())));
    let error = store.upsert(account).await.unwrap_err();
    assert!(error.to_string().contains("fix or remove it"), "{error}");
    assert!(store.remove("openai-codex-2").await.is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), corrupt);
}

#[tokio::test]
async fn one_bad_entry_keeps_the_others() {
    let dir = common::temp_dir("bad-entry");
    let path = dir.join("accounts.json");
    let good = json!({ "type": "oauth", "access": "a2", "refresh": "r2", "expires": 1 });
    let bad = json!({ "type": "oauth", "access": 5 });
    let file = json!({ "version": 1, "accounts": { "openai-codex-2": good, "openai-codex-3": bad, "anthropic-2": good } });
    std::fs::write(&path, file.to_string()).unwrap();
    let store = AccountStore::load(path.clone());
    let providers: Vec<String> = store.list().into_iter().map(|a| a.provider).collect();
    assert_eq!(providers, ["anthropic-2", "openai-codex-2"]);
    let account = Account::new(Family::Codex, 4, Some(credential("r4", now_ms())));
    store.upsert(account).await.unwrap();
    let raw: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(raw["accounts"]["openai-codex-3"], bad);
    assert_eq!(raw["accounts"]["openai-codex-2"]["refresh"], "r2");
    assert_eq!(raw["accounts"]["openai-codex-4"]["refresh"], "r4");
    assert_eq!(store.list().len(), 3);
}

#[tokio::test]
async fn file_lock_serializes_concurrent_refreshers() {
    let dir = common::temp_dir("refresh-lock");
    let path = dir.join("accounts.json");
    let stale = Account::new(Family::Anthropic, 2, Some(credential("r1", now_ms())));
    AccountStore::load(path.clone())
        .upsert(stale.clone())
        .await
        .unwrap();
    let reply = json!({ "access_token": "a2", "refresh_token": "r2", "expires_in": 28800 });
    let mock = MockHttp::start(vec![
        Reply::json(200, reply),
        Reply::json(400, json!({ "error": "invalid_grant" })),
    ])
    .await;
    let url = format!("{}/v1/oauth/token", mock.url);
    let client = reqwest::Client::new();
    let refresher = |store: AccountStore| {
        let (url, client, stale) = (url.clone(), client.clone(), stale.clone());
        tokio::spawn(async move {
            let refreshed = store
                .refresh(&stale, |current| async move {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    anthropic::refresh(&client, &url, &current.credential.as_ref().unwrap().refresh)
                        .await
                })
                .await
                .unwrap();
            assert!(refreshed.save_error.is_none());
            refreshed.account.credential.unwrap().refresh
        })
    };
    let first = refresher(AccountStore::load(path.clone()));
    let second = refresher(AccountStore::load(path.clone()));
    let (first, second) = (first.await.unwrap(), second.await.unwrap());
    assert_eq!((first.as_str(), second.as_str()), ("r2", "r2"));
    assert_eq!(mock.requests().len(), 1);
    let stored = AccountStore::load(path).get("anthropic-2").unwrap();
    assert_eq!(stored.credential.unwrap().refresh, "r2");
    assert!(!dir.join("accounts.json.lock").exists());
}
