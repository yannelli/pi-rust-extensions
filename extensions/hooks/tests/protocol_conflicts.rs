#![cfg(unix)]

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
        codex_system: Some(base.join("codex-system")),
        claude_policy: Some(base.join("claude-policy")),
        temp_dir: base.join("tmp"),
        agent_dir: Some(base.join("agent")),
    };
    Env { dir, paths, cwd }
}

/// A hook script that records its run in `<name>.ran`, then prints `out` or exits 2 with `deny`.
fn script(env: &Env, name: &str, body: &str) -> String {
    let path = env.dir.path().join("scripts").join(name);
    let ran = env.dir.path().join(format!("{name}.ran"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = format!(
        "#!/bin/sh\ncat > /dev/null\ntouch '{}'\n{body}\n",
        ran.display()
    );
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.display().to_string()
}

fn ran(env: &Env, name: &str) -> bool {
    env.dir.path().join(format!("{name}.ran")).exists()
}

fn write_json(file: &Path, doc: Value) {
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, doc.to_string()).unwrap();
}

fn bash_hooks(handler: Value) -> Value {
    json!({ "PreToolUse": [{ "matcher": "Bash", "hooks": [handler] }] })
}

fn command(cmd: &str) -> Value {
    json!({ "type": "command", "command": cmd })
}

const ALLOW: &str =
    r#"echo '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}'"#;
const DENY: &str = "echo 'global says no' >&2\nexit 2";

fn options(env: &Env, doc: Value) {
    write_json(
        &env.paths.agent_dir.clone().unwrap().join("ext/hooks.json"),
        doc,
    );
}

/// Global Claude hook denies Bash; project Claude hook allows it.
fn global_deny_project_allow(env: &Env) {
    let deny = script(env, "global", DENY);
    let allow = script(env, "project", ALLOW);
    let user = env.paths.claude_dir.join("settings.json");
    write_json(&user, json!({ "hooks": bash_hooks(command(&deny)) }));
    let project = env.cwd.join(".claude/settings.json");
    write_json(&project, json!({ "hooks": bash_hooks(command(&allow)) }));
}

async fn bash_call(env: &Env) -> Value {
    let mut host = TestHost::start(extension(env.paths.clone()));
    host.initialize().await;
    let event = json!({ "type": "tool_call", "toolCallId": "t1", "toolName": "bash",
        "input": { "command": "ls" } });
    let response = host
        .event(1, "tool_call", event, hooks_ctx(&env.cwd, false, true))
        .await;
    host.finish().await.unwrap();
    response["result"]["result"].clone()
}

#[tokio::test]
async fn merge_runs_every_layer_and_deny_wins() {
    let env = env();
    global_deny_project_allow(&env);
    let result = bash_call(&env).await;
    assert_eq!(result["block"], true, "{result}");
    assert!(ran(&env, "global") && ran(&env, "project"));
}

#[tokio::test]
async fn override_runs_only_the_most_specific_layer() {
    let env = env();
    global_deny_project_allow(&env);
    options(&env, json!({ "conflicts": "override" }));
    let result = bash_call(&env).await;
    assert_ne!(result["block"], true, "{result}");
    assert!(ran(&env, "project"));
    assert!(!ran(&env, "global"));
}

#[tokio::test]
async fn override_falls_back_when_the_specific_hook_cannot_run() {
    let env = env();
    let deny = script(&env, "global", DENY);
    let user = env.paths.claude_dir.join("settings.json");
    write_json(&user, json!({ "hooks": bash_hooks(command(&deny)) }));
    let unsupported =
        json!({ "type": "command", "command": "Write-Host x", "shell": "powershell" });
    let project = env.cwd.join(".claude/settings.json");
    write_json(&project, json!({ "hooks": bash_hooks(unsupported) }));
    options(&env, json!({ "conflicts": "override" }));
    let result = bash_call(&env).await;
    assert_eq!(result["block"], true, "{result}");
    assert!(ran(&env, "global"));
}

#[tokio::test]
async fn project_options_cannot_choose_override() {
    let env = env();
    global_deny_project_allow(&env);
    write_json(
        &env.cwd.join(".pi/ext/hooks.json"),
        json!({ "conflicts": "override" }),
    );
    let result = bash_call(&env).await;
    assert_eq!(result["block"], true, "{result}");
}

#[tokio::test]
async fn managed_claude_hooks_survive_user_disable_all_hooks() {
    let env = env();
    let deny = script(&env, "managed", DENY);
    let user_hook = script(&env, "user", ALLOW);
    let policy = env.paths.claude_policy.clone().unwrap();
    write_json(
        &policy.join("managed-settings.d/10-guard.json"),
        json!({ "hooks": bash_hooks(command(&deny)) }),
    );
    let user = env.paths.claude_dir.join("settings.json");
    write_json(
        &user,
        json!({ "disableAllHooks": true, "hooks": bash_hooks(command(&user_hook)) }),
    );
    options(&env, json!({ "conflicts": "override" }));
    let result = bash_call(&env).await;
    assert_eq!(result["block"], true, "{result}");
    assert!(ran(&env, "managed"));
    assert!(!ran(&env, "user"));
}

#[tokio::test]
async fn allow_managed_hooks_only_drops_user_hooks() {
    let env = env();
    let deny = script(&env, "user", DENY);
    let policy = env.paths.claude_policy.clone().unwrap();
    write_json(
        &policy.join("managed-settings.json"),
        json!({ "allowManagedHooksOnly": true }),
    );
    let user = env.paths.claude_dir.join("settings.json");
    write_json(&user, json!({ "hooks": bash_hooks(command(&deny)) }));
    let result = bash_call(&env).await;
    assert_ne!(result["block"], true, "{result}");
    assert!(!ran(&env, "user"));
}

#[tokio::test]
async fn codex_requirements_hooks_run_and_can_drop_user_hooks() {
    let env = env();
    let deny = script(&env, "required", DENY);
    let user_hook = script(&env, "user", ALLOW);
    let system = env.paths.codex_system.clone().unwrap();
    std::fs::create_dir_all(&system).unwrap();
    let requirements = format!(
        "allow_managed_hooks_only = true\n[[hooks.PreToolUse]]\nmatcher = \"^Bash$\"\n\
         [[hooks.PreToolUse.hooks]]\ntype = \"command\"\ncommand = \"{deny}\"\n"
    );
    std::fs::write(system.join("requirements.toml"), requirements).unwrap();
    write_json(
        &env.paths.codex_home.join("hooks.json"),
        json!({ "hooks": bash_hooks(command(&user_hook)) }),
    );
    let result = bash_call(&env).await;
    assert_eq!(result["block"], true, "{result}");
    assert!(ran(&env, "required"));
    assert!(!ran(&env, "user"));
}

/// Reads messages until the response to `id`; answers each request with `null` and returns
/// the methods and params of the requests on the way.
async fn until_response(host: &mut TestHost, id: u64) -> (Value, Vec<(String, Value)>) {
    let mut requests = Vec::new();
    loop {
        let message = host.recv().await;
        if message["id"] == id && message.get("method").is_none() {
            return (message, requests);
        }
        if let Some(method) = message["method"].as_str() {
            if message.get("id").is_some() {
                host.answer(&message["id"].clone(), Value::Null).await;
            }
            requests.push((method.to_owned(), message["params"].clone()));
        }
    }
}

fn ui_ctx(env: &Env) -> Value {
    hooks_ctx(&env.cwd, true, false)
}

fn bash_event() -> Value {
    json!({ "type": "tool_call", "toolCallId": "t1", "toolName": "bash",
        "input": { "command": "ls" } })
}

#[tokio::test]
async fn override_asks_the_next_layer_after_a_denied_prompt() {
    let env = env();
    global_deny_project_allow(&env);
    options(&env, json!({ "conflicts": "override" }));
    let mut host = TestHost::start(extension(env.paths.clone()));
    host.initialize().await;
    let params = json!({ "name": "tool_call", "event": bash_event(), "ctx": ui_ctx(&env) });
    host.request(1, "event", params).await;
    let (id, prompt) = host.expect_request("ui.select").await;
    assert!(
        prompt["title"].as_str().unwrap().contains("claude/project"),
        "{prompt}"
    );
    host.answer(&id, json!("Deny")).await;
    let (id, prompt) = host.expect_request("ui.select").await;
    assert!(
        prompt["title"].as_str().unwrap().contains("claude/global"),
        "{prompt}"
    );
    host.answer(&id, json!("Allow once")).await;
    let (response, _) = until_response(&mut host, 1).await;
    assert_eq!(response["result"]["result"]["block"], true, "{response}");
    assert!(ran(&env, "global") && !ran(&env, "project"));
    host.finish().await.unwrap();
}

#[tokio::test]
async fn a_denied_copy_of_a_handler_lets_the_other_copy_run() {
    let env = env();
    let deny = script(&env, "shared", DENY);
    let hooks = json!({ "hooks": bash_hooks(command(&deny)) });
    write_json(&env.paths.claude_dir.join("settings.json"), hooks.clone());
    write_json(&env.cwd.join(".claude/settings.json"), hooks);
    let mut host = TestHost::start(extension(env.paths.clone()));
    host.initialize().await;
    let params = json!({ "name": "tool_call", "event": bash_event(), "ctx": ui_ctx(&env) });
    host.request(1, "event", params).await;
    let (id, prompt) = host.expect_request("ui.select").await;
    assert!(prompt["title"].as_str().unwrap().contains("claude/project"));
    host.answer(&id, json!("Deny")).await;
    let (id, prompt) = host.expect_request("ui.select").await;
    assert!(prompt["title"].as_str().unwrap().contains("claude/global"));
    host.answer(&id, json!("Allow once")).await;
    let (response, _) = until_response(&mut host, 1).await;
    assert_eq!(response["result"]["result"]["block"], true, "{response}");
    host.finish().await.unwrap();
}

#[tokio::test]
async fn warn_lists_conflicts_and_disagreeing_decisions() {
    let env = env();
    global_deny_project_allow(&env);
    options(&env, json!({ "conflicts": "warn" }));
    let mut host = TestHost::start(extension(env.paths.clone()));
    host.initialize().await;
    let params = json!({ "name": "tool_call", "event": bash_event(),
        "ctx": hooks_ctx(&env.cwd, false, true) });
    host.request(1, "event", params).await;
    let (response, mut requests) = until_response(&mut host, 1).await;
    assert_eq!(response["result"]["result"]["block"], true, "{response}");
    let disagreement = "the decisions are different (allow, deny), and deny applies";
    let notices = |requests: &[(String, Value)]| -> Vec<String> {
        requests
            .iter()
            .filter(|(method, _)| method == "ui.notify")
            .filter_map(|(_, p)| p["message"].as_str().map(str::to_owned))
            .collect()
    };
    if !notices(&requests).iter().any(|m| m.contains(disagreement)) {
        let message = host.recv().await;
        host.answer(&message["id"].clone(), Value::Null).await;
        requests.push(("ui.notify".into(), message["params"].clone()));
    }
    let notices = notices(&requests);
    assert!(
        notices
            .iter()
            .any(|m| m.contains("PreToolUse Bash (Claude/pi): global, project")),
        "{notices:?}"
    );
    assert!(
        notices.iter().any(|m| m.contains(disagreement)),
        "{notices:?}"
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn pre_tool_use_stop_reason_lands_after_the_tool() {
    let env = env();
    let out = r#"{"continue":false,"stopReason":"halt after this"}"#;
    let stop = script(&env, "stop", &format!("echo '{out}'"));
    let user = env.paths.claude_dir.join("settings.json");
    write_json(&user, json!({ "hooks": bash_hooks(command(&stop)) }));
    let mut host = TestHost::start(extension(env.paths.clone()));
    host.initialize().await;
    let ctx = hooks_ctx(&env.cwd, false, true);
    let call = host.event(1, "tool_call", bash_event(), ctx.clone()).await;
    assert_ne!(call["result"]["result"]["block"], true, "{call}");
    let result = json!({ "type": "tool_result", "toolCallId": "t1", "toolName": "bash",
        "input": { "command": "ls" }, "content": [{ "type": "text", "text": "out" }],
        "isError": false });
    let params = json!({ "name": "tool_result", "event": result, "ctx": ctx });
    host.request(2, "event", params).await;
    let (response, mut requests) = until_response(&mut host, 2).await;
    let content = response["result"]["patch"]["content"].to_string();
    assert!(content.contains("halt after this"), "{response}");
    while !requests.iter().any(|(method, _)| method == "ctx.abort") {
        let message = host.recv().await;
        host.answer(&message["id"].clone(), Value::Null).await;
        requests.push((message["method"].as_str().unwrap().to_owned(), Value::Null));
    }
    host.finish().await.unwrap();
}
