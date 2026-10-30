mod common;

use common::http::{MockHttp, Reply};
use common::{codex_jwt, now_ms};
use pi_ext_subscription_router::accounts::{Account, AccountStore};
use pi_ext_subscription_router::family::Family;
use pi_ext_subscription_router::login::pasted_code;
use pi_ext_subscription_router::oauth::{
    anthropic, callback, codex, generate_pkce, parse_authorization_input, pkce_challenge,
};
use serde_json::{Value, json};

#[test]
/// Expected value: `printf %s <verifier> | openssl dgst -sha256 -binary | base64`, made URL-safe.
fn pkce_challenge_is_base64url_sha256() {
    assert_eq!(
        pkce_challenge("dBjftJeZ4CVP-mJ0vpGd3HsZbmnI2EPdyM8vBOmmYkI"),
        "aroNATaOyv00dZmrK1WYxcXAu0Vie_U3QmiXXX4mbT4"
    );
    let pkce = generate_pkce();
    assert_eq!(pkce.verifier.len(), 43);
    assert_eq!(pkce.challenge, pkce_challenge(&pkce.verifier));
}

fn query(url: &str) -> Vec<(String, String)> {
    let query = url.split_once('?').unwrap().1;
    query
        .split('&')
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap();
            (
                k.to_owned(),
                pi_ext_subscription_router::oauth::percent_decode(v),
            )
        })
        .collect()
}

#[test]
fn builds_authorize_urls() {
    let url = codex::authorize_url("chal", "st8", "pi");
    assert!(url.starts_with("https://auth.openai.com/oauth/authorize?"));
    let params = query(&url);
    let get = |k: &str| {
        params
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("client_id"), Some("app_EMoamEEZ73f0CkXaXp7hrann"));
    assert_eq!(
        get("redirect_uri"),
        Some("http://localhost:1455/auth/callback")
    );
    assert_eq!(get("scope"), Some("openid profile email offline_access"));
    assert_eq!(get("code_challenge_method"), Some("S256"));
    assert_eq!(get("state"), Some("st8"));
    assert_eq!(get("codex_cli_simplified_flow"), Some("true"));
    let url = anthropic::authorize_url("chal", "verifier");
    let params = query(&url);
    let get = |k: &str| {
        params
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    };
    assert!(url.starts_with("https://claude.ai/oauth/authorize?code=true&"));
    assert_eq!(
        get("client_id"),
        Some("9d1c250a-e61b-44d9-88ed-5944d1962f5e")
    );
    assert_eq!(
        get("redirect_uri"),
        Some("https://platform.claude.com/oauth/code/callback")
    );
    assert_eq!(get("state"), Some("verifier"));
    assert!(get("scope").unwrap().contains("user:inference"));
}

#[test]
fn parses_pasted_codes() {
    let url =
        parse_authorization_input("http://localhost:1455/auth/callback?code=abc%2F1&state=s1");
    assert_eq!(
        (url.code.as_deref(), url.state.as_deref()),
        (Some("abc/1"), Some("s1"))
    );
    let hash = parse_authorization_input(" code9#state9 ");
    assert_eq!(
        (hash.code.as_deref(), hash.state.as_deref()),
        (Some("code9"), Some("state9"))
    );
    let query = parse_authorization_input("code=q&state=r");
    assert_eq!(query.code.as_deref(), Some("q"));
    let bare = parse_authorization_input("plain");
    assert_eq!((bare.code.as_deref(), bare.state), (Some("plain"), None));
    assert_eq!(
        pasted_code(Some("c#s".into()), "s").unwrap(),
        ("c".into(), Some("s".into()))
    );
    assert_eq!(
        pasted_code(Some("c#other".into()), "s").unwrap_err(),
        "State mismatch"
    );
    assert_eq!(pasted_code(None, "s").unwrap_err(), "Login cancelled");
}

#[tokio::test]
async fn callback_server_returns_the_code() {
    let mut server = callback::start(0, "/auth/callback", "st").await.unwrap();
    let base = format!("http://127.0.0.1:{}", server.port);
    let client = reqwest::Client::new();
    let wrong = client.get(format!("{base}/other")).send().await.unwrap();
    assert_eq!(wrong.status(), 404);
    let ok = client
        .get(format!("{base}/auth/callback?code=xyz&state=st"))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    assert_eq!(server.wait().await.unwrap(), "xyz");
    let mut mismatch = callback::start(0, "/cb", "good").await.unwrap();
    let url = format!("http://127.0.0.1:{}/cb?code=1&state=bad", mismatch.port);
    assert_eq!(client.get(url).send().await.unwrap().status(), 400);
    assert_eq!(mismatch.wait().await.unwrap_err(), "State mismatch");
}

fn form(body: &str) -> Vec<(String, String)> {
    body.split('&')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| {
            (
                k.to_owned(),
                pi_ext_subscription_router::oauth::percent_decode(v),
            )
        })
        .collect()
}

#[tokio::test]
async fn codex_refresh_posts_form_and_reads_account() {
    let access = codex_jwt("acct-7");
    let reply = json!({ "access_token": access, "refresh_token": "r2", "expires_in": 3600 });
    let mock = MockHttp::start(vec![Reply::json(200, reply)]).await;
    let client = reqwest::Client::new();
    let credential = codex::refresh(&client, &format!("{}/oauth/token", mock.url), "r1")
        .await
        .unwrap();
    assert_eq!(credential.refresh, "r2");
    assert_eq!(credential.account_id.as_deref(), Some("acct-7"));
    assert!((credential.expires - now_ms() - 3_600_000).abs() < 5_000);
    let request = &mock.requests()[0];
    assert_eq!(
        (request.method.as_str(), request.path.as_str()),
        ("POST", "/oauth/token")
    );
    let fields = form(&request.body);
    assert!(fields.contains(&("grant_type".into(), "refresh_token".into())));
    assert!(fields.contains(&("refresh_token".into(), "r1".into())));
    assert!(fields.contains(&("client_id".into(), codex::CLIENT_ID.into())));
}

#[tokio::test]
async fn anthropic_refresh_posts_json_and_reports_errors() {
    let reply =
        json!({ "access_token": "sk-ant-oat01-a", "refresh_token": "r2", "expires_in": 28800 });
    let mock = MockHttp::start(vec![
        Reply::json(200, reply),
        Reply::json(400, json!({ "error": "invalid_grant" })),
    ])
    .await;
    let client = reqwest::Client::new();
    let url = format!("{}/v1/oauth/token", mock.url);
    let credential = anthropic::refresh(&client, &url, "r1").await.unwrap();
    assert_eq!(credential.access, "sk-ant-oat01-a");
    assert!((credential.expires - now_ms() - (28_800_000 - 300_000)).abs() < 5_000);
    let body: Value = serde_json::from_str(&mock.requests()[0].body).unwrap();
    assert_eq!(
        body,
        json!({ "grant_type": "refresh_token", "client_id": anthropic::client_id(), "refresh_token": "r1" })
    );
    let error = anthropic::refresh(&client, &url, "r2").await.unwrap_err();
    assert!(
        error.contains("(400)") && error.contains("invalid_grant"),
        "{error}"
    );
}

#[test]
fn account_store_writes_private_file_and_numbers_slots() {
    let dir = common::temp_dir("accounts");
    let path = dir.join("ext/subscription-router/accounts.json");
    let store = AccountStore::load(path.clone());
    assert_eq!(store.next_slot(Family::Codex), 2);
    let credential = pi_ext_subscription_router::oauth::Credential {
        access: "a".into(),
        refresh: "r".into(),
        expires: now_ms() + 60_000,
        account_id: Some("x".into()),
    };
    store
        .upsert(Account::new(Family::Codex, 2, credential.clone()))
        .unwrap();
    store
        .upsert(Account::new(Family::Anthropic, 2, credential))
        .unwrap();
    assert_eq!(store.next_slot(Family::Codex), 3);
    let reloaded = AccountStore::load(path.clone());
    let providers: Vec<String> = reloaded.list().into_iter().map(|a| a.provider).collect();
    assert_eq!(providers, ["anthropic-2", "openai-codex-2"]);
    assert!(
        reloaded
            .get("openai-codex-2")
            .unwrap()
            .needs_refresh(now_ms())
    );
    let raw: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(raw["accounts"]["openai-codex-2"]["type"], "oauth");
    assert_eq!(raw["accounts"]["openai-codex-2"]["accountId"], "x");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(reloaded.remove("openai-codex-2").unwrap());
    assert_eq!(AccountStore::load(path).list().len(), 1);
}
