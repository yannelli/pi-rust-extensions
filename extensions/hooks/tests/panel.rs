#![cfg(unix)]

mod common;

use std::path::PathBuf;

use common::{TestHost, hooks_ctx};
use pi_ext_hooks::{Paths, extension};
use serde_json::{Value, json};

struct Env {
    _dir: tempfile::TempDir,
    paths: Paths,
    cwd: PathBuf,
    agent: PathBuf,
}

const DENY: &str = "cat > /dev/null; echo 'blocked by hook' >&2; exit 2";

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_path_buf();
    let cwd = base.join("repo");
    std::fs::create_dir_all(cwd.join(".git")).unwrap();
    let paths = Paths {
        claude_dir: base.join("claude"),
        codex_home: base.join("codex"),
        temp_dir: base.join("tmp"),
        agent_dir: Some(base.join("agent")),
    };
    let settings = json!({ "hooks": { "PreToolUse": [{ "matcher": "Bash",
        "hooks": [{ "type": "command", "command": DENY }] }] } });
    std::fs::create_dir_all(&paths.claude_dir).unwrap();
    std::fs::write(paths.claude_dir.join("settings.json"), settings.to_string()).unwrap();
    Env {
        agent: base.join("agent"),
        _dir: dir,
        paths,
        cwd,
    }
}

fn ctx(env: &Env, project_trusted: bool) -> Value {
    let mut ctx = hooks_ctx(&env.cwd, true, true);
    ctx["isProjectTrusted"] = json!(project_trusted);
    if !project_trusted {
        ctx["terminalWidth"] = json!(40);
    }
    ctx
}

fn text(lines: &Value) -> String {
    let lines = lines.as_array().unwrap().iter().filter_map(Value::as_str);
    pi_ext_sdk::style::strip(&lines.collect::<Vec<_>>().join("\n"))
}

/// Runs `/hooks` and returns the panel request id and surface.
async fn open(host: &mut TestHost, id: u64, ctx: Value) -> (Value, Value, String) {
    host.request(
        id,
        "command.run",
        json!({ "name": "hooks", "args": "", "ctx": ctx }),
    )
    .await;
    let (panel, params) = host.expect_request("ui.custom").await;
    (panel, params["surface"].clone(), text(&params["lines"]))
}

async fn key(host: &mut TestHost, surface: &Value, data: &str) -> String {
    host.notify("ui.key", json!({ "surface": surface, "data": data }))
        .await;
    text(&host.expect_notification("ui.update").await["lines"])
}

async fn close(host: &mut TestHost, id: u64, panel: &Value, surface: &Value) {
    host.notify("ui.key", json!({ "surface": surface, "data": "\u{1b}" }))
        .await;
    host.expect_notification("ui.close").await;
    host.answer(panel, Value::Null).await;
    host.expect_response(id).await;
}

async fn blocked(host: &mut TestHost, id: u64, env: &Env) -> bool {
    let event = json!({ "type": "tool_call", "toolCallId": "t1", "toolName": "bash",
        "input": { "command": "echo hi" } });
    let response = host.event(id, "tool_call", event, ctx(env, true)).await;
    response["result"]["result"]["block"] == true
}

#[tokio::test]
async fn space_disables_and_enables_in_the_running_session() {
    let env = env();
    let mut host = TestHost::start(extension(env.paths.clone()));
    host.initialize().await;
    assert!(blocked(&mut host, 1, &env).await);

    let (panel, surface, lines) = open(&mut host, 2, ctx(&env, true)).await;
    assert!(lines.contains("● PreToolUse"), "{lines}");
    let lines = key(&mut host, &surface, " ").await;
    assert!(lines.contains("○ PreToolUse"), "{lines}");
    assert!(lines.contains("Disabled PreToolUse Bash"), "{lines}");
    close(&mut host, 2, &panel, &surface).await;

    let global = env.agent.join("ext/hooks.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&global).unwrap()).unwrap();
    assert_eq!(doc["disabled"].as_array().map(Vec::len), Some(1));
    let settings = std::fs::read_to_string(env.paths.claude_dir.join("settings.json")).unwrap();
    assert!(settings.contains(DENY));
    assert!(!blocked(&mut host, 3, &env).await);

    let (panel, surface, _) = open(&mut host, 4, ctx(&env, true)).await;
    let lines = key(&mut host, &surface, " ").await;
    assert!(lines.contains("Enabled PreToolUse Bash"), "{lines}");
    close(&mut host, 4, &panel, &surface).await;
    assert!(blocked(&mut host, 5, &env).await);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn add_writes_the_chosen_scope_and_refuses_untrusted_projects() {
    let env = env();
    let mut host = TestHost::start(extension(env.paths.clone()));
    host.initialize().await;
    let (panel, surface, lines) = open(&mut host, 1, ctx(&env, false)).await;
    assert!(lines.contains("Project not trusted"), "{lines}");
    assert!(lines.lines().all(|l| l.chars().count() <= 40), "{lines}");
    key(&mut host, &surface, "a").await;
    let lines = key(&mut host, &surface, "p").await;
    assert!(
        lines.contains("Cannot add: project is not trusted"),
        "{lines}"
    );
    assert!(!env.cwd.join(".pi").exists());

    key(&mut host, &surface, "a").await;
    host.notify("ui.key", json!({ "surface": surface, "data": "g" }))
        .await;
    host.expect_notification("ui.close").await;
    host.answer(&panel, Value::Null).await;
    let (id, params) = host.expect_request("ui.select").await;
    assert_eq!(params["options"][0], "PreToolUse");
    host.answer(&id, json!("Stop")).await;
    let (id, _) = host.expect_request("ui.input").await;
    host.answer(&id, json!("")).await;
    let (id, _) = host.expect_request("ui.input").await;
    host.answer(&id, json!("echo done")).await;
    let (panel, params) = host.expect_request("ui.custom").await;
    let lines = text(&params["lines"]);
    assert!(lines.contains("Added a global hook"), "{lines}");
    assert!(lines.contains("→ ● Stop"), "{lines}");
    close(&mut host, 1, &panel, &params["surface"]).await;

    let doc: Value =
        serde_json::from_str(&std::fs::read_to_string(env.agent.join("ext/hooks.json")).unwrap())
            .unwrap();
    assert_eq!(
        doc["hooks"],
        json!({ "Stop": [{ "hooks": [{ "type": "command", "command": "echo done" }] }] })
    );
    let state = std::fs::read_to_string(env.agent.join("ext/hooks/state.json")).unwrap();
    let state: Value = serde_json::from_str(&state).unwrap();
    assert_eq!(state["trusted"].as_array().map(Vec::len), Some(1));
    host.finish().await.unwrap();
}

#[tokio::test]
async fn remove_asks_then_edits_the_settings_file() {
    let env = env();
    let mut host = TestHost::start(extension(env.paths.clone()));
    host.initialize().await;
    let (panel, surface, _) = open(&mut host, 1, ctx(&env, true)).await;
    let lines = key(&mut host, &surface, "d").await;
    assert!(
        lines.contains("Remove PreToolUse Bash from settings.json?"),
        "{lines}"
    );
    let lines = key(&mut host, &surface, "y").await;
    assert!(lines.contains("Removed PreToolUse Bash"), "{lines}");
    assert!(lines.contains("No hooks found"), "{lines}");
    close(&mut host, 1, &panel, &surface).await;
    let doc: Value = serde_json::from_str(
        &std::fs::read_to_string(env.paths.claude_dir.join("settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(doc, json!({ "hooks": {} }));
    assert!(!blocked(&mut host, 2, &env).await);
    host.finish().await.unwrap();
}
