//! Account management without dialogs: `/router login start|finish` and `/router remove --yes`.

mod common;

use std::sync::Arc;

use common::http::MockHttp;
use common::{TestHost, ctx, temp_dir};
use pi_ext_subscription_router::state::State;
use pi_ext_subscription_router::usage::Urls;
use serde_json::{Value, json};

async fn host(name: &str, accounts: Option<Value>) -> (TestHost, std::path::PathBuf) {
    let mock = MockHttp::start(vec![]).await;
    let dir = temp_dir(name);
    std::fs::create_dir_all(dir.join("ext/subscription-router")).unwrap();
    if let Some(accounts) = accounts {
        std::fs::write(
            dir.join("ext/subscription-router/accounts.json"),
            accounts.to_string(),
        )
        .unwrap();
    }
    let urls = Urls {
        codex: format!("{}/wham/usage", mock.url),
        anthropic: format!("{}/api/oauth/usage", mock.url),
    };
    let state = Arc::new(State::with_urls(&dir, urls));
    let mut host = TestHost::start(pi_ext_subscription_router::extension(state));
    host.initialize_with(dir.to_str().unwrap(), &["providerAuth"])
        .await;
    (host, dir)
}

/// Runs `/router <args>` and returns the notices it sent, as `(type, message)`.
async fn run(host: &mut TestHost, id: u64, args: &str) -> Vec<(String, String)> {
    host.request(
        id,
        "command.run",
        json!({ "name": "router", "args": args, "ctx": ctx(None) }),
    )
    .await;
    let (_, seen) = host.serve_until(id, |_, _| Value::Null).await;
    seen.into_iter()
        .filter(|(method, _)| method == "ui.notify")
        .map(|(_, params)| {
            let kind = params["type"]
                .as_str()
                .or(params["notifyType"].as_str())
                .unwrap_or("info")
                .to_owned();
            (
                kind,
                params["message"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn login_start_answers_a_url_and_reserves_the_next_account() {
    let accounts = json!({ "version": 1, "accounts": { "anthropic-2": { "type": "pi" } } });
    let (mut host, dir) = host("login-start", Some(accounts)).await;
    let notices = run(&mut host, 1, "login start claude").await;
    let started: Value = serde_json::from_str(&notices[0].1).unwrap();
    assert_eq!(started["provider"], "anthropic-3");
    assert_eq!(started["name"], "Anthropic #3");
    assert!(
        started["url"]
            .as_str()
            .unwrap()
            .starts_with("https://claude.ai/oauth/authorize?"),
        "{started}"
    );
    let id = started["id"].as_str().unwrap().to_owned();
    assert!(
        dir.join(format!("ext/subscription-router/logins/{id}.json"))
            .exists()
    );

    // A second login while the first is open takes the account after it.
    let second: Value =
        serde_json::from_str(&run(&mut host, 2, "login start claude").await[0].1).unwrap();
    assert_eq!(second["provider"], "anthropic-4");

    // A pasted code for another login is refused; an unknown id too.
    let wrong = run(
        &mut host,
        3,
        &format!("login finish {id} abc#not-the-state"),
    )
    .await;
    assert_eq!(
        wrong,
        vec![("error".to_owned(), "State mismatch".to_owned())]
    );
    let unknown = run(&mut host, 4, "login finish 0123abcd code").await;
    assert!(
        unknown[0].1.contains("expired or does not exist"),
        "{unknown:?}"
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn remove_needs_yes_and_only_takes_router_accounts() {
    let accounts = json!({ "version": 1, "accounts": { "openai-codex-2": { "type": "pi" } } });
    let (mut host, dir) = host("remove", Some(accounts)).await;
    let refused = run(&mut host, 1, "remove openai-codex-2").await;
    assert!(refused[0].1.contains("--yes"), "{refused:?}");
    let base = run(&mut host, 2, "remove openai-codex --yes").await;
    assert!(
        base[0].1.contains("not an account the router added"),
        "{base:?}"
    );
    let removed: Value =
        serde_json::from_str(&run(&mut host, 3, "remove openai-codex-2 --yes").await[0].1).unwrap();
    assert_eq!(
        removed,
        json!({ "removed": true, "provider": "openai-codex-2" })
    );
    let stored =
        std::fs::read_to_string(dir.join("ext/subscription-router/accounts.json")).unwrap();
    assert!(!stored.contains("openai-codex-2"), "{stored}");
    host.finish().await.unwrap();
}
