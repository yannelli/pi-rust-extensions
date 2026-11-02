#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;

use common::{TestHost, hooks_ctx};
use pi_ext_hooks::{Paths, extension};
use serde_json::{Value, json};

struct Env {
    dir: tempfile::TempDir,
    paths: Paths,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    std::fs::create_dir_all(base.join("repo/.git")).unwrap();
    let paths = Paths {
        claude_dir: base.join("claude"),
        codex_home: base.join("codex"),
        codex_system: None,
        temp_dir: base.join("tmp"),
        agent_dir: Some(base.join("agent")),
    };
    Env { dir, paths }
}

fn hooks(env: &Env, event: &str, matcher: Option<&str>, body: &str) {
    let path = env.dir.path().join(format!("{event}.sh"));
    std::fs::write(&path, format!("#!/bin/sh\ncat > /dev/null\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let group = json!([{ "matcher": matcher, "hooks": [{ "type": "command", "command": path }] }]);
    std::fs::create_dir_all(&env.paths.claude_dir).unwrap();
    std::fs::write(
        env.paths.claude_dir.join("settings.json"),
        json!({ "hooks": { event: group } }).to_string(),
    )
    .unwrap();
}

/// Sends an event and answers host requests (`[]` for the branch, else `null`) until it replies.
async fn event(host: &mut TestHost, id: u64, name: &str, event: Value, env: &Env) -> Value {
    let ctx = hooks_ctx(&env.dir.path().join("repo"), false, true);
    host.request(
        id,
        "event",
        json!({ "name": name, "event": event, "ctx": ctx }),
    )
    .await;
    loop {
        let message = host.recv().await;
        if message.get("id") == Some(&json!(id)) && message.get("method").is_none() {
            return message["result"].clone();
        }
        if let Some(method) = message.get("method").and_then(Value::as_str) {
            let answer = if method == "session.getBranch" {
                json!([])
            } else {
                Value::Null
            };
            if let Some(request) = message.get("id") {
                host.answer(&request.clone(), answer).await;
            }
        }
    }
}

async fn start(env: &Env) -> TestHost {
    let mut host = TestHost::start(extension(env.paths.clone()));
    host.initialize().await;
    host
}

#[tokio::test]
async fn prompt_expansion_blocks_by_command_name() {
    let env = env();
    hooks(
        &env,
        "UserPromptExpansion",
        Some("deploy"),
        "echo 'needs approval' >&2\nexit 2",
    );
    let mut host = start(&env).await;
    let ev = json!({ "type": "prompt_expansion", "kind": "skill", "name": "deploy", "args": "", "prompt": "/deploy", "text": "x" });
    let result = event(&mut host, 1, "prompt_expansion", ev, &env).await;
    assert_eq!(
        result["result"],
        json!({ "action": "block", "reason": "needs approval" })
    );
    let other = json!({ "type": "prompt_expansion", "name": "review", "prompt": "/review" });
    assert_eq!(
        event(&mut host, 2, "prompt_expansion", other, &env).await,
        json!({})
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn elicitation_hook_answers_the_form() {
    let env = env();
    let out = r#"{"hookSpecificOutput":{"hookEventName":"Elicitation","action":"accept","content":{"user":"a"}}}"#;
    hooks(&env, "Elicitation", Some("auth"), &format!("echo '{out}'"));
    let mut host = start(&env).await;
    let ev =
        json!({ "type": "mcp_elicitation", "server": "auth", "message": "Who?", "mode": "form" });
    let result = event(&mut host, 1, "mcp_elicitation", ev, &env).await;
    assert_eq!(
        result["result"],
        json!({ "action": "accept", "content": { "user": "a" } })
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn task_created_hook_blocks_the_todo_call() {
    let env = env();
    hooks(
        &env,
        "TaskCreated",
        None,
        "jq -r .task_subject >&2 2>/dev/null || true\necho 'needs a ticket' >&2\nexit 2",
    );
    let mut host = start(&env).await;
    let call = json!({ "type": "tool_call", "toolCallId": "t1", "toolName": "todo",
        "input": { "action": "create", "subject": "Fix it" } });
    let result = event(&mut host, 1, "tool_call", call, &env).await;
    assert_eq!(result["result"]["block"], true);
    assert!(
        result["result"]["reason"]
            .as_str()
            .unwrap()
            .contains("needs a ticket")
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn post_tool_use_gets_duration_and_mcp_server() {
    let env = env();
    let out = env.dir.path().join("post.json");
    hooks(&env, "PostToolUse", None, "true");
    let script = format!("#!/bin/sh\ncat > {}\n", out.display());
    std::fs::write(env.dir.path().join("PostToolUse.sh"), script).unwrap();
    let mut host = start(&env).await;
    let call = json!({ "type": "tool_call", "toolCallId": "t1", "toolName": "mcp__fs__read", "input": {} });
    event(&mut host, 1, "tool_call", call, &env).await;
    let result = json!({ "type": "tool_result", "toolCallId": "t1", "toolName": "mcp__fs__read",
        "input": {}, "content": [{ "type": "text", "text": "ok" }], "isError": false });
    event(&mut host, 2, "tool_result", result, &env).await;
    let sent: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert!(sent["duration_ms"].is_u64(), "{sent}");
    assert_eq!(sent["mcp_server"]["name"], "fs");
    host.finish().await.unwrap();
}

#[tokio::test]
async fn mcp_tool_hook_blocks_through_mcp_call_tool() {
    let env = env();
    let handler = json!({ "type": "mcp_tool", "server": "scanner", "tool": "scan",
        "input": { "cmd": "${tool_input.command}" } });
    std::fs::create_dir_all(&env.paths.claude_dir).unwrap();
    std::fs::write(
        env.paths.claude_dir.join("settings.json"),
        json!({ "hooks": { "PreToolUse": [{ "matcher": "Bash", "hooks": [handler] }] } })
            .to_string(),
    )
    .unwrap();
    let mut host = start(&env).await;
    let ctx = hooks_ctx(&env.dir.path().join("repo"), false, true);
    let call = json!({ "type": "tool_call", "toolCallId": "t1", "toolName": "bash", "input": { "command": "rm -rf x" } });
    host.request(
        1,
        "event",
        json!({ "name": "tool_call", "event": call, "ctx": ctx }),
    )
    .await;
    let mut seen = None;
    let result = loop {
        let message = host.recv().await;
        if message.get("id") == Some(&json!(1)) && message.get("method").is_none() {
            break message["result"].clone();
        }
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            continue;
        };
        let answer = if method == "mcp.callTool" {
            seen = Some(message["params"].clone());
            let text = r#"{"decision":"block","reason":"scan failed"}"#;
            json!({ "content": [{ "type": "text", "text": text }], "isError": false })
        } else {
            Value::Null
        };
        if let Some(id) = message.get("id") {
            host.answer(&id.clone(), answer).await;
        }
    };
    let params = seen.expect("mcp.callTool was called");
    assert_eq!(
        (&params["server"], &params["tool"]),
        (&json!("scanner"), &json!("scan"))
    );
    assert_eq!(params["args"], json!({ "cmd": "rm -rf x" }));
    assert_eq!(
        result["result"],
        json!({ "block": true, "reason": "scan failed" })
    );
    host.finish().await.unwrap();
}
