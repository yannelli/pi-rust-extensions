//! Protocol-level runs against a fake `pi` that replays a scripted JSON event stream.

#![cfg(unix)]

mod common;

use std::time::Duration;

use common::{TestHost, ctx, dirs, scripted_events};
use serde_json::{Value, json};

fn execute(params: Value, cwd: &std::path::Path) -> Value {
    json!({ "toolCallId": "c1", "name": "subagent", "params": params, "ctx": ctx(cwd) })
}

fn scope_events(host: &TestHost) -> Vec<Value> {
    let emits = host.seen_of("events.emit").into_iter();
    emits
        .filter(|e| e["channel"] == "todo:scope")
        .map(|e| e["data"].clone())
        .collect()
}

#[tokio::test]
async fn foreground_run_streams_and_inherits_model() {
    let d = dirs(&scripted_events("Found main at src/main.rs:3"), 0);
    let mut host = TestHost::start(pi_ext_subagents::extension());
    host.initialize(&d.agent, &d.cwd).await;
    let params = json!({ "agent": "explore", "task": "Find main" });
    host.request(1, "tool.execute", execute(params, &d.cwd))
        .await;
    let response = host.until_response(1).await;
    let result = &response["result"];
    assert_eq!(result["content"][0]["text"], "Found main at src/main.rs:3");
    assert!(result.get("isError").is_none(), "{result}");
    let run = &result["details"]["runs"][0];
    assert_eq!(run["status"], "completed");
    assert_eq!(run["turns"], 1);
    assert_eq!(run["tools"].as_array().unwrap().len(), 2);
    let updates = host.seen_of("tool.update");
    assert!(!updates.is_empty());
    let last = updates.last().unwrap()["partial"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(last.contains("explore: Find main"), "{last}");

    let args = std::fs::read_to_string(d.fake.join("args")).unwrap();
    let args: Vec<&str> = args.lines().collect();
    let flag = |name: &str| args.iter().position(|a| *a == name).map(|i| args[i + 1]);
    assert_eq!(flag("--model"), Some("mock/parent-model"));
    assert_eq!(flag("--thinking"), Some("high"));
    assert_eq!(flag("--tools"), Some("read,grep,find,ls,todo"));
    let env = std::fs::read_to_string(d.fake.join("env")).unwrap();
    let id = run["id"].as_str().unwrap();
    assert!(env.contains(&format!("PI_TODO_SCOPE={id}")), "{env}");
    let cwd = std::fs::read_to_string(d.fake.join("cwd")).unwrap();
    assert_eq!(
        std::fs::canonicalize(cwd.trim()).unwrap(),
        std::fs::canonicalize(&d.cwd).unwrap()
    );

    let scopes = scope_events(&host);
    assert_eq!(scopes.first().unwrap()["status"], "running");
    assert!(
        scopes
            .iter()
            .any(|s| s["state"]["tasks"][0]["subject"] == "scan")
    );
    assert_eq!(scopes.last().unwrap()["status"], "completed");
    assert!(scopes.iter().all(|s| s["scope"] == id));

    let lifecycle: Vec<Value> = host
        .seen_of("events.emit")
        .into_iter()
        .filter(|e| {
            e["channel"]
                .as_str()
                .is_some_and(|c| c.starts_with("subagents:"))
        })
        .collect();
    assert_eq!(lifecycle.len(), 2, "{lifecycle:?}");
    assert_eq!(lifecycle[0]["channel"], "subagents:start");
    assert_eq!(
        lifecycle[0]["data"],
        json!({ "agentId": id, "agentType": "explore" })
    );
    assert_eq!(lifecycle[1]["channel"], "subagents:stop");
    assert_eq!(lifecycle[1]["data"]["agentId"], id);
    assert_eq!(lifecycle[1]["data"]["agentType"], "explore");
    assert_eq!(lifecycle[1]["data"]["status"], "completed");
    assert_eq!(
        lifecycle[1]["data"]["lastAssistantMessage"],
        "Found main at src/main.rs:3"
    );
    host.finish().await;
}

#[tokio::test]
async fn background_run_reports_with_message_and_widget() {
    let d = dirs(&scripted_events("Background answer"), 0);
    let mut host = TestHost::start(pi_ext_subagents::extension());
    host.initialize(&d.agent, &d.cwd).await;
    let params = json!({ "agent": "advisor", "task": "Review the plan", "async": true });
    host.request(1, "tool.execute", execute(params, &d.cwd))
        .await;
    let response = host.until_response(1).await;
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("Started 1 background subagent run"),
        "{text}"
    );
    let message = host.until_method("session.sendMessage").await;
    assert_eq!(message["customType"], "subagent-result");
    assert_eq!(message["triggerTurn"], true);
    assert_eq!(message["deliverAs"], "followUp");
    assert!(
        message["content"]
            .as_str()
            .unwrap()
            .contains("Background answer")
    );
    let widgets = host.seen_of("ui.setWidget");
    assert!(widgets.iter().any(|w| w["key"] == "subagents"
        && w["lines"].as_array().is_some_and(|l| {
            l.iter()
                .any(|x| x.as_str().unwrap_or("").contains("advisor"))
        })));
    assert!(
        widgets.last().unwrap()["lines"].is_null(),
        "widget clears when runs end"
    );
    host.finish().await;
}

#[tokio::test]
async fn parallel_tasks_return_sections() {
    let d = dirs(&scripted_events("ok"), 0);
    let mut host = TestHost::start(pi_ext_subagents::extension());
    host.initialize(&d.agent, &d.cwd).await;
    let params = json!({ "tasks": [{ "task": "one" }, { "agent": "build", "task": "two" }] });
    host.request(1, "tool.execute", execute(params, &d.cwd))
        .await;
    let response = host.until_response(1).await;
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("## [1] explore: one") && text.contains("## [2] build: two"),
        "{text}"
    );
    assert_eq!(
        response["result"]["details"]["runs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    host.finish().await;
}

#[tokio::test]
async fn cancel_kills_the_child() {
    let d = dirs(&scripted_events("never"), 30);
    let mut host = TestHost::start(pi_ext_subagents::extension());
    host.initialize(&d.agent, &d.cwd).await;
    let params = json!({ "agent": "explore", "task": "Slow" });
    host.request(1, "tool.execute", execute(params, &d.cwd))
        .await;
    host.until_method("tool.update").await;
    let pid = std::fs::read_to_string(d.fake.join("pid"))
        .unwrap()
        .trim()
        .to_owned();
    host.notify("$/cancelRequest", json!({ "id": 1 })).await;
    let response = host.until_response(1).await;
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(text.starts_with("Cancelled."), "{response}");
    assert_eq!(
        response["result"]["details"]["runs"][0]["status"],
        "cancelled"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(!alive.success(), "fake pi {pid} still runs");
    host.finish().await;
}

#[tokio::test]
async fn shutdown_kills_a_child_that_ignores_sigterm() {
    let d = dirs(&scripted_events("never"), 30);
    let script = std::fs::read_to_string(d.fake.join("fake-pi")).unwrap();
    let script = script.replacen("#!/bin/sh\n", "#!/bin/sh\ntrap '' TERM\n", 1);
    std::fs::write(d.fake.join("fake-pi"), script).unwrap();
    let mut host = TestHost::start(pi_ext_subagents::extension());
    host.initialize(&d.agent, &d.cwd).await;
    let params = json!({ "agent": "explore", "task": "Slow", "async": true });
    host.request(1, "tool.execute", execute(params, &d.cwd))
        .await;
    host.until_response(1).await;
    let pid_file = d.fake.join("pid");
    while !pid_file.exists() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let pid = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .to_owned();
    host.finish().await;
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(!alive.success(), "fake pi {pid} still runs after shutdown");
}

#[test]
fn cwd_parameter_stays_inside_the_session_cwd() {
    use pi_ext_subagents::tool::child_cwd;
    let root = pi_ext_e2e::TempDir::new().unwrap();
    let cwd = root.path().join("work");
    std::fs::create_dir_all(cwd.join("sub")).unwrap();
    assert_eq!(child_cwd(&cwd, "sub").unwrap(), cwd.join("sub"));
    assert_eq!(child_cwd(&cwd, "sub/../sub").unwrap(), cwd.join("sub"));
    let inside = cwd.join("sub").display().to_string();
    assert_eq!(child_cwd(&cwd, &inside).unwrap(), cwd.join("sub"));
    for rel in ["..", "../work2", "/etc", "sub/../../x"] {
        assert!(child_cwd(&cwd, rel).is_err(), "{rel}");
    }
    std::os::unix::fs::symlink(root.path(), cwd.join("up")).unwrap();
    assert!(child_cwd(&cwd, "up").is_err());
}

#[tokio::test]
async fn unknown_agent_and_prompt_section() {
    let d = dirs(&scripted_events("x"), 0);
    let mut host = TestHost::start(pi_ext_subagents::extension());
    host.initialize(&d.agent, &d.cwd).await;
    let params = json!({ "agent": "nope", "task": "x" });
    host.request(1, "tool.execute", execute(params, &d.cwd))
        .await;
    let response = host.until_response(1).await;
    assert_eq!(response["result"]["isError"], true);
    assert!(
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Available: explore, build, advisor")
    );
    let event = json!({ "type": "before_agent_start", "prompt": "hi",
        "systemPromptOptions": { "sections": {}, "selectedTools": ["read", "subagent"] } });
    host.request(
        2,
        "event",
        json!({ "name": "before_agent_start", "event": event, "ctx": ctx(&d.cwd) }),
    )
    .await;
    let response = host.until_response(2).await;
    let section = response["result"]["patch"]["systemPromptOptions"]["sections"]["subagents"]
        .as_str()
        .unwrap();
    assert!(
        section.contains("`explore`") && section.contains("`advisor`"),
        "{section}"
    );
    host.finish().await;
}

#[tokio::test]
async fn runs_panel_lists_runs_and_closes() {
    let d = dirs(&scripted_events("done"), 0);
    let mut host = TestHost::start(pi_ext_subagents::extension());
    host.initialize(&d.agent, &d.cwd).await;
    host.request(1, "tool.execute", execute(json!({ "task": "t" }), &d.cwd))
        .await;
    host.until_response(1).await;
    host.request(
        2,
        "command.run",
        json!({ "name": "agents", "args": "runs", "ctx": ctx(&d.cwd) }),
    )
    .await;
    let custom = host.until_method("ui.custom").await;
    let lines = custom["lines"].as_array().unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l.as_str().unwrap().contains("explore: t")),
        "{custom}"
    );
    let surface = custom["surface"].clone();
    host.notify("ui.key", json!({ "surface": surface, "data": "\r" }))
        .await;
    let mut detail = false;
    for _ in 0..10 {
        let update = host.until_method("ui.update").await;
        let lines = update["lines"].as_array().unwrap().clone();
        if lines
            .iter()
            .any(|l| l.as_str().unwrap().contains("Output:"))
        {
            detail = true;
            break;
        }
    }
    assert!(detail, "enter opens the run output");
    host.notify("ui.key", json!({ "surface": surface, "data": "\u{1b}" }))
        .await;
    host.notify("ui.key", json!({ "surface": surface, "data": "q" }))
        .await;
    let close = host.until_method("ui.close").await;
    assert_eq!(close["surface"], surface);
    host.finish().await;
}
