mod common;

use common::TestHost;
use pi_ext_iterm2::extension;
use pi_ext_iterm2::reporter::Terminal;
use pi_ext_iterm2::sequences::{
    DONE, WORKING, badge, current_dir, request_attention, tab_color, user_var,
};
use serde_json::{Value, json};

fn ctx(run_state: &str) -> Value {
    json!({
        "cwd": "/work/node-cli", "hasUI": true, "flags": {}, "sessionName": "quotes",
        "model": { "provider": "openai", "id": "gpt-5" },
        "footer": { "gitBranch": "main", "runState": run_state }
    })
}

async fn event(host: &mut TestHost, id: u64, name: &str, run_state: &str) {
    let params = json!({ "name": name, "event": { "type": name }, "ctx": ctx(run_state) });
    host.request(id, "event", params).await;
}

/// Answers `settings.get` requests until the first `ui.writeTerminal`, which it returns.
async fn answer_settings_then_write(host: &mut TestHost, terminal: Value) -> Value {
    loop {
        let message = host.recv().await;
        match message["method"].as_str() {
            Some("settings.get") => {
                assert_eq!(message["params"]["namespace"], "terminal");
                host.answer(&message["id"], terminal.clone()).await;
            }
            Some("ui.writeTerminal") => return message,
            _ => panic!("unexpected message {message}"),
        }
    }
}

async fn expect_write(host: &mut TestHost, data: &str, tmux: bool) {
    let (id, params) = host.expect_request("ui.writeTerminal").await;
    assert_eq!(params["data"], data);
    assert_eq!(params["tmuxPassthrough"], tmux);
    host.answer(&id, json!(true)).await;
}

#[tokio::test]
async fn state_changes_write_iterm2_sequences() {
    let mut host = TestHost::start(extension(Terminal {
        iterm2: true,
        tmux: true,
    }));
    host.initialize().await;
    event(&mut host, 1, "agent_start", "working").await;
    let terminal = json!({ "iterm2": true, "notifyMinSeconds": 0 });
    let first = answer_settings_then_write(&mut host, terminal).await;
    assert_eq!(first["params"]["data"], tab_color(Some(WORKING)));
    assert_eq!(first["params"]["tmuxPassthrough"], true);
    host.answer(&first["id"], json!(true)).await;
    expect_write(&mut host, &user_var("pi_state", "working"), true).await;
    expect_write(&mut host, &badge("π quotes"), true).await;
    expect_write(&mut host, &current_dir("/work/node-cli".as_ref()), true).await;
    expect_write(&mut host, &user_var("pi_session", "quotes"), true).await;
    expect_write(&mut host, &user_var("pi_project", "node-cli"), true).await;
    expect_write(&mut host, &user_var("pi_model", "gpt-5"), true).await;
    host.expect_response(1).await;

    event(&mut host, 2, "turn_start", "working").await;
    host.expect_response(2).await;

    event(&mut host, 3, "agent_end", "done").await;
    expect_write(&mut host, request_attention(), true).await;
    expect_write(&mut host, &tab_color(Some(DONE)), true).await;
    expect_write(&mut host, &user_var("pi_state", "done"), true).await;
    host.expect_response(3).await;

    host.request(4, "shutdown", json!({})).await;
    expect_write(&mut host, &tab_color(None), true).await;
    expect_write(&mut host, &badge(""), true).await;
    host.expect_response(4).await;
    host.finish().await.unwrap();
}

#[tokio::test]
async fn features_from_the_old_setting_limit_what_is_written() {
    let mut host = TestHost::start(extension(Terminal {
        iterm2: true,
        tmux: false,
    }));
    host.initialize().await;
    event(&mut host, 1, "session_start", "idle").await;
    let terminal = json!({ "iterm2": { "tabColor": false, "userVars": false, "cwd": false } });
    let first = answer_settings_then_write(&mut host, terminal).await;
    assert_eq!(first["params"]["data"], badge("π quotes"));
    assert_eq!(first["params"]["tmuxPassthrough"], false);
    host.answer(&first["id"], json!(true)).await;
    host.expect_response(1).await;
    host.finish().await.unwrap();
}

#[tokio::test]
async fn outside_iterm2_nothing_is_written() {
    let mut host = TestHost::start(extension(Terminal {
        iterm2: false,
        tmux: false,
    }));
    let init = host.initialize().await;
    assert_eq!(init["commands"][0]["name"], "terminal-setup");
    event(&mut host, 1, "agent_start", "working").await;
    host.expect_response(1).await;
    host.request(2, "shutdown", json!({})).await;
    host.expect_response(2).await;
    host.finish().await.unwrap();
}
