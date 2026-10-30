mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use common::{TestHost, hooks_ctx};
use pi_ext_hooks::{Paths, extension};
use serde_json::{Value, json};

struct Env {
    dir: tempfile::TempDir,
    paths: Paths,
    cwd: PathBuf,
}

fn env(hooks: Value) -> Env {
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
    std::fs::create_dir_all(&paths.claude_dir).unwrap();
    std::fs::write(
        paths.claude_dir.join("settings.json"),
        json!({ "hooks": hooks }).to_string(),
    )
    .unwrap();
    Env { dir, paths, cwd }
}

fn script(env: &Env, name: &str, body: &str) -> String {
    let path = env.dir.path().join(name);
    std::fs::write(&path, format!("#!/bin/sh\ncat > /dev/null\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.display().to_string()
}

fn group(matcher: &str, command: &str) -> Value {
    json!([{ "matcher": matcher, "hooks": [{ "type": "command", "command": command }] }])
}

fn rewrite_hooks(env: &Env, hooks: Value) {
    std::fs::write(
        env.paths.claude_dir.join("settings.json"),
        json!({ "hooks": hooks }).to_string(),
    )
    .unwrap();
}

async fn start(env: &Env) -> TestHost {
    let mut host = TestHost::start(extension(env.paths.clone()));
    host.initialize().await;
    host
}

#[tokio::test]
async fn session_start_context_reaches_before_agent_start_once() {
    let mut env = env(json!({}));
    let env_line = "echo 'export FOO=1' >> \"$CLAUDE_ENV_FILE\"";
    let cmd = script(
        &env,
        "start.sh",
        &format!("{env_line}\necho \"project $CLAUDE_PROJECT_DIR\""),
    );
    rewrite_hooks(&env, json!({ "SessionStart": group("startup", &cmd) }));
    env.cwd = env.cwd.canonicalize().unwrap();
    let mut host = start(&env).await;
    let ctx = hooks_ctx(&env.cwd, false, true);
    let start = json!({ "type": "session_start", "reason": "startup" });
    assert_eq!(
        host.event(1, "session_start", start, ctx.clone()).await["result"],
        json!({})
    );
    let before = json!({ "type": "before_agent_start", "prompt": "hi", "systemPromptOptions": {} });
    let response = host
        .event(2, "before_agent_start", before.clone(), ctx.clone())
        .await;
    let message = &response["result"]["result"]["message"];
    assert_eq!(message["customType"], "hooks-context");
    assert_eq!(message["content"], format!("project {}", env.cwd.display()));
    assert_eq!(message["display"], false);
    assert_eq!(
        host.event(3, "before_agent_start", before, ctx.clone())
            .await["result"],
        json!({})
    );

    let call = json!({ "type": "tool_call", "toolCallId": "t1", "toolName": "bash", "input": { "command": "echo $FOO" } });
    let response = host.event(4, "tool_call", call, ctx).await;
    let env_file = env.dir.path().join("agent/ext/hooks/env/s1.sh");
    assert_eq!(
        std::fs::read_to_string(&env_file).unwrap(),
        "export FOO=1\n"
    );
    assert_eq!(
        response["result"]["patch"]["input"]["command"],
        format!(". '{}'; echo $FOO", env_file.display())
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn post_tool_use_appends_feedback_and_held_context() {
    let env = env(json!({}));
    let pre = script(
        &env,
        "pre.sh",
        r#"echo '{"hookSpecificOutput":{"additionalContext":"pre note"}}'"#,
    );
    let post = script(
        &env,
        "post.sh",
        r#"echo '{"decision":"block","reason":"lint failed"}'"#,
    );
    rewrite_hooks(
        &env,
        json!({ "PreToolUse": group("Read", &pre), "PostToolUse": group("Read|Write", &post) }),
    );
    let mut host = start(&env).await;
    let ctx = hooks_ctx(&env.cwd, false, true);
    let call = json!({ "type": "tool_call", "toolCallId": "t9", "toolName": "read", "input": { "path": "a.rs" } });
    assert_eq!(
        host.event(1, "tool_call", call, ctx.clone()).await["result"],
        json!({})
    );
    let result = json!({
        "type": "tool_result", "toolCallId": "t9", "toolName": "read", "input": { "path": "a.rs" },
        "content": [{ "type": "text", "text": "fn main() {}" }], "isError": false
    });
    let response = host.event(2, "tool_result", result, ctx).await;
    let texts: Vec<&str> = response["result"]["patch"]["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["fn main() {}", "pre note", "lint failed"]);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn pre_compact_block_cancels() {
    let env = env(json!({}));
    let cmd = script(&env, "compact.sh", "echo 'not now' >&2\nexit 2");
    rewrite_hooks(&env, json!({ "PreCompact": group("manual", &cmd) }));
    let mut host = start(&env).await;
    let ctx = hooks_ctx(&env.cwd, false, true);
    let auto =
        json!({ "type": "session_before_compact", "reason": "threshold", "willRetry": false });
    assert_eq!(
        host.event(1, "session_before_compact", auto, ctx.clone())
            .await["result"],
        json!({})
    );
    let manual =
        json!({ "type": "session_before_compact", "reason": "manual", "willRetry": false });
    host.request(
        2,
        "event",
        json!({ "name": "session_before_compact", "event": manual, "ctx": ctx }),
    )
    .await;
    let (id, params) = host.expect_request("ui.notify").await;
    assert_eq!(params["message"], "Compaction cancelled by hook: not now");
    host.answer(&id, Value::Null).await;
    assert_eq!(
        host.expect_response(2).await["result"],
        json!({ "result": { "cancel": true } })
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn hooks_list_command_notifies_listing() {
    let env = env(json!({}));
    let cmd = script(&env, "x.sh", "exit 0");
    rewrite_hooks(&env, json!({ "PreToolUse": group("Bash", &cmd) }));
    let mut host = start(&env).await;
    let ctx = hooks_ctx(&env.cwd, false, true);
    host.request(
        1,
        "command.run",
        json!({ "name": "hooks", "args": "list", "ctx": ctx }),
    )
    .await;
    let (id, params) = host.expect_request("ui.notify").await;
    let text = params["message"].as_str().unwrap();
    assert!(text.contains("trustAll off (flag on)"), "{text}");
    assert!(text.contains("PreToolUse"), "{text}");
    assert!(
        text.contains(&format!(
            "[claude/global] Bash  {cmd}  (trustAll)  never run"
        )),
        "{text}"
    );
    host.answer(&id, Value::Null).await;
    assert_eq!(host.expect_response(1).await["result"], Value::Null);
    host.finish().await.unwrap();
}
