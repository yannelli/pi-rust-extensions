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
        codex_system: None,
        claude_policy: None,
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

const DOWN: &str = "\u{1b}[B";
const UP: &str = "\u{1b}[A";
const RIGHT: &str = "\u{1b}[C";

#[tokio::test]
async fn right_arrow_disables_and_enables_in_the_running_session() {
    let env = env();
    let mut host = TestHost::start(extension(env.paths.clone()));
    host.initialize().await;
    assert!(blocked(&mut host, 1, &env).await);

    let (panel, surface, lines) = open(&mut host, 2, ctx(&env, true)).await;
    assert!(lines.contains("● PreToolUse"), "{lines}");
    key(&mut host, &surface, DOWN).await;
    let lines = key(&mut host, &surface, RIGHT).await;
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
    key(&mut host, &surface, DOWN).await;
    let lines = key(&mut host, &surface, RIGHT).await;
    assert!(lines.contains("Enabled PreToolUse Bash"), "{lines}");
    close(&mut host, 4, &panel, &surface).await;
    assert!(blocked(&mut host, 5, &env).await);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn add_runs_in_the_panel_and_untrusted_projects_offer_global_only() {
    let env = env();
    let mut host = TestHost::start(extension(env.paths.clone()));
    host.initialize().await;
    let (panel, surface, lines) = open(&mut host, 1, ctx(&env, false)).await;
    assert!(lines.contains("Project not trusted"), "{lines}");
    assert!(lines.lines().all(|l| l.chars().count() <= 40), "{lines}");
    let lines = key(&mut host, &surface, "\r").await;
    assert!(lines.contains("Add a command hook"), "{lines}");
    assert!(
        lines.contains("[global]") && !lines.contains("project]"),
        "{lines}"
    );
    let stop = pi_ext_hooks_events()
        .iter()
        .position(|e| *e == "Stop")
        .unwrap();
    for _ in 0..stop {
        key(&mut host, &surface, DOWN).await;
    }
    key(&mut host, &surface, "\r").await;
    key(&mut host, &surface, "\r").await;
    key(&mut host, &surface, "echo done").await;
    let lines = key(&mut host, &surface, "\r").await;
    assert!(lines.contains("Added a global Stop hook"), "{lines}");
    assert!(lines.contains("→ ● Stop"), "{lines}");
    close(&mut host, 1, &panel, &surface).await;

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

/// The event list of the add form, in order.
fn pi_ext_hooks_events() -> [&'static str; 16] {
    [
        "PreToolUse",
        "PostToolUse",
        "PostToolUseFailure",
        "PostToolBatch",
        "PermissionRequest",
        "UserPromptSubmit",
        "Stop",
        "StopFailure",
        "SubagentStart",
        "SubagentStop",
        "SessionStart",
        "SessionEnd",
        "PreCompact",
        "PostCompact",
        "PreModelSwitch",
        "PostModelSwitch",
    ]
}

#[tokio::test]
async fn remove_asks_for_a_second_enter_then_edits_the_settings_file() {
    let env = env();
    let mut host = TestHost::start(extension(env.paths.clone()));
    host.initialize().await;
    let (panel, surface, _) = open(&mut host, 1, ctx(&env, true)).await;
    key(&mut host, &surface, DOWN).await;
    key(&mut host, &surface, "\r").await;
    key(&mut host, &surface, UP).await;
    let lines = key(&mut host, &surface, "\r").await;
    assert!(
        lines.contains("Remove PreToolUse Bash from settings.json?"),
        "{lines}"
    );
    let lines = key(&mut host, &surface, "\r").await;
    assert!(lines.contains("Removed PreToolUse Bash"), "{lines}");
    assert!(lines.contains("no hooks found"), "{lines}");
    close(&mut host, 1, &panel, &surface).await;
    let doc: Value = serde_json::from_str(
        &std::fs::read_to_string(env.paths.claude_dir.join("settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(doc, json!({ "hooks": {} }));
    assert!(!blocked(&mut host, 2, &env).await);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn panel_fits_the_terminal_height_from_ctx_and_resize() {
    let env = env();
    let mut host = TestHost::start(extension(env.paths.clone()));
    host.initialize().await;
    let mut ctx = ctx(&env, true);
    ctx["terminalWidth"] = json!(80);
    ctx["terminalHeight"] = json!(8);
    let (panel, surface, _) = open(&mut host, 1, ctx).await;
    let lines = key(&mut host, &surface, DOWN).await;
    assert!(lines.lines().count() <= 5, "{lines}");
    assert!(lines.contains("→ ● PreToolUse"), "{lines}");
    let sizes = [(json!(80), json!(40), 37), (Value::Null, json!(7), 4)];
    for (width, height, max_lines) in sizes {
        let mut params = json!({ "surface": surface, "height": height });
        if !width.is_null() {
            params["width"] = width;
        }
        host.notify("ui.resize", params).await;
        let lines = text(&host.expect_notification("ui.update").await["lines"]);
        assert!(lines.lines().count() <= max_lines, "{lines}");
        assert!(lines.contains("→ ● PreToolUse"), "{lines}");
        assert!(lines.contains("esc close"), "{lines}");
    }
    close(&mut host, 1, &panel, &surface).await;
    host.finish().await.unwrap();
}

#[tokio::test]
async fn settings_tab_sets_the_conflict_mode_in_place() {
    let env = env();
    let project = env.cwd.join(".claude/settings.json");
    std::fs::create_dir_all(project.parent().unwrap()).unwrap();
    let settings = json!({ "hooks": { "PreToolUse": [{ "matcher": "Bash",
        "hooks": [{ "type": "command", "command": "echo project" }] }] } });
    std::fs::write(&project, settings.to_string()).unwrap();
    let mut host = TestHost::start(extension(env.paths.clone()));
    host.initialize().await;
    let (panel, surface, lines) = open(&mut host, 1, ctx(&env, true)).await;
    assert!(lines.contains("[conflict]"), "{lines}");
    let lines = key(&mut host, &surface, "\u{1b}[Z").await;
    assert!(lines.contains("[Settings ("), "{lines}");
    assert!(lines.contains("Conflicts between layers"), "{lines}");
    key(&mut host, &surface, "\u{1b}[H").await;
    for _ in 0..5 {
        key(&mut host, &surface, DOWN).await;
    }
    host.notify("ui.key", json!({ "surface": surface, "data": RIGHT }))
        .await;
    let (id, notice) = host.expect_request("ui.notify").await;
    assert!(
        notice["message"]
            .as_str()
            .unwrap()
            .contains("have conflicts")
    );
    host.answer(&id, Value::Null).await;
    let lines = text(&host.expect_notification("ui.update").await["lines"]);
    assert!(lines.contains("Set conflicts = \"warn\""), "{lines}");
    let lines = key(&mut host, &surface, RIGHT).await;
    assert!(lines.contains("conflicts: override"), "{lines}");
    assert!(lines.contains("Set conflicts"), "{lines}");
    let lines = key(&mut host, &surface, "\t").await;
    assert!(lines.contains("[shadowed]"), "{lines}");
    close(&mut host, 1, &panel, &surface).await;
    let doc: Value =
        serde_json::from_str(&std::fs::read_to_string(env.agent.join("ext/hooks.json")).unwrap())
            .unwrap();
    assert_eq!(doc["conflicts"], "override");
    host.finish().await.unwrap();
}
