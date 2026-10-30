mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::{TestHost, hooks_ctx};
use pi_ext_hooks::{Paths, extension};
use serde_json::{Value, json};

struct Env {
    dir: tempfile::TempDir,
    paths: Paths,
    cwd: PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    let cwd = base.join("repo");
    std::fs::create_dir_all(cwd.join(".git")).unwrap();
    let paths = Paths {
        claude_dir: base.join("claude"),
        codex_home: base.join("codex"),
        temp_dir: base.join("tmp"),
        agent_dir: Some(base.join("agent")),
    };
    Env { dir, paths, cwd }
}

/// Writes an executable hook script and returns its path.
fn script(env: &Env, name: &str, body: &str) -> String {
    let path = env.dir.path().join("scripts").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.display().to_string()
}

fn write_hooks(file: &Path, event: &str, matcher: Option<&str>, command: &str) {
    let group = match matcher {
        Some(m) => json!({ "matcher": m, "hooks": [{ "type": "command", "command": command }] }),
        None => json!({ "hooks": [{ "type": "command", "command": command }] }),
    };
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, json!({ "hooks": { event: [group] } }).to_string()).unwrap();
}

async fn start(env: &Env) -> TestHost {
    let mut host = TestHost::start(extension(env.paths.clone()));
    host.initialize().await;
    host
}

fn tool_call(command: Value) -> Value {
    json!({ "type": "tool_call", "toolCallId": "t1", "toolName": "bash", "input": command })
}

#[tokio::test]
async fn pre_tool_use_deny_blocks_tool_call() {
    let env = env();
    let stdin = env.dir.path().join("stdin.json");
    let cmd = script(
        &env,
        "deny.sh",
        &format!(
            "cat > '{}'\necho 'rm is not allowed' >&2\nexit 2",
            stdin.display()
        ),
    );
    write_hooks(
        &env.paths.claude_dir.join("settings.json"),
        "PreToolUse",
        Some("Bash"),
        &cmd,
    );
    let mut host = start(&env).await;
    let ctx = hooks_ctx(&env.cwd, false, true);
    let response = host
        .event(
            1,
            "tool_call",
            tool_call(json!({ "command": "rm -rf x" })),
            ctx,
        )
        .await;
    let result = &response["result"]["result"];
    assert_eq!(result["block"], true, "{response}");
    assert_eq!(
        result["reason"],
        format!("PreToolUse:Bash hook error: [{cmd}]: rm is not allowed")
    );
    let sent: Value = serde_json::from_str(&std::fs::read_to_string(stdin).unwrap()).unwrap();
    assert_eq!(sent["hook_event_name"], "PreToolUse");
    assert_eq!(sent["tool_name"], "Bash");
    assert_eq!(sent["tool_input"]["command"], "rm -rf x");
    assert_eq!(sent["tool_use_id"], "t1");
    assert_eq!(sent["session_id"], "s1");
    assert_eq!(sent["permission_mode"], "default");
    host.finish().await.unwrap();
}

#[tokio::test]
async fn updated_input_patches_the_event() {
    let env = env();
    let out = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":{"command":"ls -la"}}}"#;
    let cmd = script(
        &env,
        "rewrite.sh",
        &format!("cat > /dev/null\necho '{out}'"),
    );
    write_hooks(
        &env.paths.codex_home.join("hooks.json"),
        "PreToolUse",
        Some("^shell$"),
        &cmd,
    );
    let mut host = start(&env).await;
    let ctx = hooks_ctx(&env.cwd, false, true);
    let response = host
        .event(
            1,
            "tool_call",
            tool_call(json!({ "command": "ls", "timeout": 5 })),
            ctx,
        )
        .await;
    assert_eq!(
        response["result"],
        json!({ "patch": { "input": { "command": "ls -la", "timeout": null } } })
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn stop_block_sends_follow_up() {
    let env = env();
    let stdin = env.dir.path().join("stop.json");
    let out = r#"{"decision":"block","reason":"run the tests first"}"#;
    let cmd = script(
        &env,
        "stop.sh",
        &format!("cat > '{}'\necho '{out}'", stdin.display()),
    );
    write_hooks(&env.paths.codex_home.join("hooks.json"), "Stop", None, &cmd);
    let mut host = start(&env).await;
    let ctx = hooks_ctx(&env.cwd, false, true);
    let messages = json!([
        { "role": "user", "content": "hi" },
        { "role": "assistant", "content": [{ "type": "text", "text": "All done." }], "stopReason": "stop" }
    ]);
    let end = host
        .event(
            1,
            "agent_end",
            json!({ "type": "agent_end", "messages": messages }),
            ctx.clone(),
        )
        .await;
    assert_eq!(end["result"], json!({}));
    host.request(2, "event", json!({ "name": "agent_settled", "event": { "type": "agent_settled", "aborted": false }, "ctx": ctx }))
        .await;
    let (id, params) = host.expect_request("session.sendMessage").await;
    assert_eq!(
        params,
        json!({ "customType": "hooks-stop", "content": "run the tests first", "display": false, "deliverAs": "followUp", "triggerTurn": true })
    );
    host.answer(&id, Value::Null).await;
    assert_eq!(host.expect_response(2).await["result"], json!({}));
    let sent: Value = serde_json::from_str(&std::fs::read_to_string(&stdin).unwrap()).unwrap();
    assert_eq!(sent["stop_hook_active"], false);
    assert_eq!(sent["last_assistant_message"], "All done.");
    assert!(sent.get("model").is_some(), "Codex stdin has model");
    host.finish().await.unwrap();
}

#[tokio::test]
async fn user_prompt_submit_block_returns_handled() {
    let env = env();
    let cmd = script(
        &env,
        "prompt.sh",
        "cat > /dev/null\necho 'no secrets in prompts' >&2\nexit 2",
    );
    write_hooks(
        &env.dir.path().join("agent/ext/hooks.json"),
        "UserPromptSubmit",
        None,
        &cmd,
    );
    let mut host = start(&env).await;
    let ctx = hooks_ctx(&env.cwd, false, true);
    let event = json!({ "type": "input", "text": "my key is x", "source": "interactive" });
    host.request(
        1,
        "event",
        json!({ "name": "input", "event": event, "ctx": ctx }),
    )
    .await;
    let (id, params) = host.expect_request("ui.notify").await;
    assert_eq!(
        params["message"],
        "Prompt blocked by hook: no secrets in prompts"
    );
    host.answer(&id, Value::Null).await;
    assert_eq!(
        host.expect_response(1).await["result"],
        json!({ "result": { "action": "handled" } })
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn prompt_context_transforms_input() {
    let env = env();
    let cmd = script(&env, "ctx.sh", "cat > /dev/null\necho 'Branch: main'");
    write_hooks(
        &env.paths.claude_dir.join("settings.json"),
        "UserPromptSubmit",
        None,
        &cmd,
    );
    let mut host = start(&env).await;
    let event = json!({ "type": "input", "text": "hi", "source": "rpc" });
    let response = host
        .event(1, "input", event, hooks_ctx(&env.cwd, false, true))
        .await;
    assert_eq!(
        response["result"]["result"],
        json!({ "action": "transform", "text": "hi\n\nBranch: main" })
    );
    let skipped = json!({ "type": "input", "text": "hi", "source": "extension" });
    assert_eq!(
        host.event(2, "input", skipped, hooks_ctx(&env.cwd, false, true))
            .await["result"],
        json!({})
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn untrusted_hook_asks_and_remembers() {
    let env = env();
    let cmd = script(&env, "ok.sh", "cat > /dev/null");
    write_hooks(
        &env.paths.claude_dir.join("settings.json"),
        "PreToolUse",
        Some("Bash"),
        &cmd,
    );
    let mut host = start(&env).await;
    let ctx = hooks_ctx(&env.cwd, true, false);
    host.request(1, "event", json!({ "name": "tool_call", "event": tool_call(json!({ "command": "ls" })), "ctx": ctx.clone() }))
        .await;
    let (id, params) = host.expect_request("ui.select").await;
    assert_eq!(
        params["options"],
        json!(["Allow once", "Always allow", "Deny"])
    );
    assert!(
        params["title"]
            .as_str()
            .unwrap()
            .contains("[claude/global] PreToolUse Bash")
    );
    host.answer(&id, json!("Always allow")).await;
    assert_eq!(host.expect_response(1).await["result"], json!({}));
    let state: Value = serde_json::from_str(
        &std::fs::read_to_string(env.dir.path().join("agent/ext/hooks/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["trusted"].as_array().unwrap().len(), 1);
    let again = host
        .event(2, "tool_call", tool_call(json!({ "command": "ls" })), ctx)
        .await;
    assert_eq!(
        again["result"],
        json!({}),
        "trusted hook runs without asking"
    );
    host.finish().await.unwrap();
}
