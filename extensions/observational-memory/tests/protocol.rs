mod common;
mod fixtures;

use common::{TestHost, ctx};
use fixtures::*;
use pi_ext_observational_memory::extension;
use serde_json::{Value, json};

fn quiet_ctx() -> Value {
    json!({ "cwd": "/work", "hasUI": false, "flags": {}, "isProjectTrusted": false })
}

#[tokio::test]
async fn registers_recall_commands_and_events() {
    let mut host = TestHost::start(extension());
    let init = host.initialize().await;
    let tool = &init["tools"][0];
    assert_eq!(tool["name"], "recall");
    assert_eq!(tool["label"], "Recall memory evidence");
    assert_eq!(
        tool["parameters"]["properties"]["id"]["pattern"],
        "^[a-f0-9]{12}$"
    );
    assert_eq!(tool["promptGuidelines"].as_array().unwrap().len(), 6);
    assert!(
        tool["promptSnippet"]
            .as_str()
            .unwrap()
            .starts_with("Use recall(<id>)")
    );
    let commands: Vec<&str> = init["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(commands, ["om:status", "om:view"]);
    assert_eq!(
        init["events"],
        json!([
            "agent_start",
            "turn_end",
            "agent_settled",
            "session_before_compact"
        ])
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn turn_end_runs_observer_and_appends_entry() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let branch = json!(conversation(3, 16_000));
    host.request(
        5,
        "event",
        json!({ "name": "turn_end", "event": { "type": "turn_end" }, "ctx": quiet_ctx() }),
    )
    .await;
    let reply = json!({ "observations": [{ "timestamp": "2026-10-29 10:00",
        "content": "User pasted three long inputs.", "relevance": "low", "sourceEntryIds": ["u0", "u2"] }] }).to_string();
    let mut completions = Vec::new();
    let seen = host
        .drive(
            |method, params| match method {
                "settings.get" => json!({ "showWorkerNotifications": false }),
                "session.getBranch" => branch.clone(),
                "llm.complete" => {
                    completions.push(params.clone());
                    json!({ "message": {}, "text": reply })
                }
                _ => Value::Null,
            },
            |message| message["method"] == "session.appendEntry",
        )
        .await;
    assert!(
        seen.iter()
            .any(|m| m["id"] == 5 && m["result"] == json!({}))
    );
    let append = &seen.last().unwrap()["params"];
    assert_eq!(append["customType"], "om.observations.recorded");
    assert_eq!(append["data"]["coversUpToId"], "u2");
    let observation = &append["data"]["observations"][0];
    assert_eq!(observation["sourceEntryIds"], json!(["u0", "u2"]));
    assert_eq!(observation["relevance"], "low");
    assert_eq!(observation["id"].as_str().unwrap().len(), 12);
    assert!(observation["tokenCount"].as_u64().unwrap() > 0);
    assert_eq!(completions.len(), 1);
    assert!(
        completions[0]["systemPrompt"]
            .as_str()
            .unwrap()
            .starts_with("You are the observation agent")
    );
    assert_eq!(completions[0]["maxTokens"], 32_000);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn before_compact_returns_memory_summary() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let o1 = observation("User stated they use Postgres.", "critical", &["u0"]);
    let entries = json!([
        user("u0", "I use Postgres"),
        observations_entry("m1", std::slice::from_ref(&o1), "u0"),
        user("u1", "next")
    ]);
    let event = json!({ "type": "session_before_compact", "reason": "manual", "willRetry": false,
        "preparation": { "firstKeptEntryId": "u1", "tokensBefore": 1234 }, "branchEntries": entries });
    host.request(
        6,
        "event",
        json!({ "name": "session_before_compact", "event": event, "ctx": quiet_ctx() }),
    )
    .await;
    let seen = host
        .drive(
            |_, _| json!({}),
            |m| m["id"] == 6 && m.get("method").is_none(),
        )
        .await;
    let compaction = &seen.last().unwrap()["result"]["result"]["compaction"];
    assert_eq!(compaction["firstKeptEntryId"], "u1");
    assert_eq!(compaction["tokensBefore"], 1234);
    let summary = compaction["summary"].as_str().unwrap();
    assert!(summary.starts_with("These are condensed memories from earlier in this session."));
    assert!(summary.contains(&format!("## Observations\n[{}]", o1.id)));
    assert_eq!(compaction["details"]["type"], "om.folded");
    assert_eq!(compaction["details"]["fullFold"], false);

    let empty = json!({ "type": "session_before_compact", "preparation": { "firstKeptEntryId": "u1", "tokensBefore": 1 },
        "branchEntries": [user("u0", "hi"), user("u1", "there")] });
    host.request(
        7,
        "event",
        json!({ "name": "session_before_compact", "event": empty, "ctx": quiet_ctx() }),
    )
    .await;
    assert_eq!(host.expect_response(7).await["result"], json!({}));
    host.finish().await.unwrap();
}

#[tokio::test]
async fn recall_unknown_and_invalid_ids() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    host.request(8, "tool.execute", json!({ "toolCallId": "c1", "name": "recall", "params": { "id": "abcdefabcdef" }, "ctx": ctx() })).await;
    let (id, params) = host.expect_request("session.getBranch").await;
    assert_eq!(params, json!({}));
    host.answer(&id, json!([user("u0", "hi")])).await;
    let result = host.expect_response(8).await["result"].clone();
    assert_eq!(
        result["content"][0]["text"],
        "No observation or reflection with id abcdefabcdef was found on the current branch."
    );
    assert_eq!(result["details"]["status"], "not_found");

    host.request(
        9,
        "tool.execute",
        json!({ "toolCallId": "c2", "name": "recall", "params": { "id": "XYZ" }, "ctx": ctx() }),
    )
    .await;
    let result = host.expect_response(9).await["result"].clone();
    assert_eq!(
        result["content"][0]["text"],
        "Memory id must be 12 lowercase hex characters. Received: XYZ"
    );
    assert_eq!(result["details"]["status"], "invalid_id");
    host.finish().await.unwrap();
}

#[tokio::test]
async fn view_copies_through_exec_and_notifies() {
    let agent_dir = std::env::temp_dir().join(format!("om-view-{}", std::process::id()));
    let mut host = TestHost::start(extension());
    host.initialize_with(&agent_dir.display().to_string()).await;
    host.request(
        10,
        "command.run",
        json!({ "name": "om:view", "args": "full", "ctx": ctx() }),
    )
    .await;
    let mut execs = Vec::new();
    let seen = host
        .drive(
            |method, params| match method {
                "session.getBranch" => json!([]),
                "exec" => {
                    execs.push(params.clone());
                    json!({ "stdout": "", "stderr": "", "code": 0, "killed": false })
                }
                _ => Value::Null,
            },
            |m| m["method"] == "ui.notify",
        )
        .await;
    let message = seen.last().unwrap()["params"]["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(message.starts_with("── Reflections ──\nNo recorded reflections."));
    assert!(message.ends_with("Copied /om:view output to clipboard."));
    assert_eq!(execs[0]["command"], "sh");
    assert!(
        execs[0]["args"][1]
            .as_str()
            .unwrap()
            .starts_with("pbcopy < '")
    );
    let state_dir = agent_dir.join("ext/observational-memory");
    assert_eq!(std::fs::read_dir(&state_dir).unwrap().count(), 0);
    assert_eq!(host.expect_response(10).await["result"], Value::Null);
    host.finish().await.unwrap();
    std::fs::remove_dir_all(agent_dir).ok();
}

#[tokio::test]
async fn agent_settled_compacts_over_threshold_when_idle() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let branch = json!(conversation(3, 120_000));
    let event = json!({ "name": "agent_settled", "event": { "type": "agent_settled" }, "ctx": quiet_ctx() });
    host.request(11, "event", event).await;
    let seen = host
        .drive(
            |method, _| match method {
                "settings.get" => json!({}),
                "session.getBranch" => branch.clone(),
                "ctx.isIdle" => json!(true),
                _ => Value::Null,
            },
            |m| m["method"] == "ctx.compact",
        )
        .await;
    let methods: Vec<&str> = seen.iter().filter_map(|m| m["method"].as_str()).collect();
    let idle = methods.iter().position(|m| *m == "ctx.isIdle").unwrap();
    assert!(idle < methods.len() - 1);
    assert_eq!(seen.last().unwrap()["params"], json!({}));
    host.finish().await.unwrap();
}

fn event(name: &str, ctx: Value) -> Value {
    json!({ "name": name, "event": { "type": name }, "ctx": ctx })
}

fn observer_reply() -> String {
    json!({ "observations": [{ "timestamp": "2026-10-29 10:00",
        "content": "User pasted inputs.", "relevance": "low", "sourceEntryIds": ["u0"] }] })
    .to_string()
}

/// Runs `/om:status` and returns every request method seen until its response.
async fn methods_until_status(host: &mut TestHost, id: u64, branch: &Value) -> Vec<String> {
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    host.request(
        id,
        "command.run",
        json!({ "name": "om:status", "args": "", "ctx": quiet_ctx() }),
    )
    .await;
    let seen = host
        .drive(
            |method, _| match method {
                "session.getBranch" => branch.clone(),
                "ctx.isIdle" => json!(true),
                _ => Value::Null,
            },
            |m| m["id"] == id && m.get("method").is_none(),
        )
        .await;
    seen.iter()
        .filter_map(|m| m["method"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn passive_mode_stops_workers_and_auto_compaction() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let branch = json!(conversation(3, 400_000));
    host.request(20, "event", event("turn_end", quiet_ctx()))
        .await;
    host.request(21, "event", event("agent_settled", quiet_ctx()))
        .await;
    host.drive(
        |method, _| match method {
            "settings.get" => json!({ "passive": true }),
            _ => Value::Null,
        },
        |m| m["method"] == "settings.get",
    )
    .await;
    let methods = methods_until_status(&mut host, 22, &branch).await;
    assert_eq!(
        methods.iter().filter(|m| *m == "session.getBranch").count(),
        1,
        "{methods:?}"
    );
    assert!(
        !methods
            .iter()
            .any(|m| matches!(m.as_str(), "llm.complete" | "ctx.compact" | "ctx.isIdle")),
        "{methods:?}"
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn concurrent_turn_ends_start_one_pipeline() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let branch = json!(conversation(3, 16_000));
    host.request(30, "event", event("turn_end", quiet_ctx()))
        .await;
    host.request(31, "event", event("turn_end", quiet_ctx()))
        .await;
    let reply = observer_reply();
    let mut methods = Vec::new();
    host.drive(
        |method, _| {
            methods.push(method.to_owned());
            match method {
                "settings.get" => json!({ "showWorkerNotifications": false }),
                "session.getBranch" => branch.clone(),
                "llm.complete" => json!({ "message": {}, "text": reply }),
                _ => Value::Null,
            }
        },
        |m| m["method"] == "session.appendEntry",
    )
    .await;
    let after = methods_until_status(&mut host, 32, &branch).await;
    assert_eq!(methods.iter().filter(|m| *m == "llm.complete").count(), 1);
    assert_eq!(methods.iter().filter(|m| *m == "settings.get").count(), 1);
    assert!(!after.iter().any(|m| m == "llm.complete"), "{after:?}");
    host.finish().await.unwrap();
}

#[tokio::test]
async fn compaction_resumes_when_consolidation_finishes() {
    deferred_compaction(true).await;
}

#[tokio::test]
async fn deferred_compaction_stops_when_the_agent_becomes_busy() {
    deferred_compaction(false).await;
}

async fn deferred_compaction(idle: bool) {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let branch = json!(conversation(3, 400_000));
    host.request(40, "event", event("turn_end", quiet_ctx()))
        .await;
    let held = loop {
        let message = host.recv().await;
        let (Some(method), Some(id)) = (message["method"].as_str(), message.get("id")) else {
            continue;
        };
        let id = id.clone();
        match method {
            "llm.complete" => break id,
            "settings.get" => {
                host.answer(&id, json!({ "showWorkerNotifications": false }))
                    .await
            }
            "session.getBranch" => host.answer(&id, branch.clone()).await,
            _ => host.answer(&id, Value::Null).await,
        }
    };

    host.request(41, "event", event("agent_settled", quiet_ctx()))
        .await;
    assert_eq!(host.expect_response(41).await["result"], json!({}));
    let methods = methods_until_status(&mut host, 42, &branch).await;
    assert!(
        !methods
            .iter()
            .any(|m| m == "ctx.compact" || m == "ctx.isIdle"),
        "{methods:?}"
    );
    host.answer(
        &held,
        json!({ "message": {}, "text": "{\"observations\": []}" }),
    )
    .await;
    host.drive(
        |method, _| match method {
            "session.getBranch" => branch.clone(),
            "ctx.isIdle" => json!(idle),
            _ => Value::Null,
        },
        |m| m["method"] == if idle { "ctx.compact" } else { "ctx.isIdle" },
    )
    .await;
    let later = methods_until_status(&mut host, 43, &branch).await;
    assert!(!later.iter().any(|m| m == "ctx.compact"), "{later:?}");
    host.finish().await.unwrap();
}

#[tokio::test]
async fn untrusted_project_config_is_ignored() {
    let dir = std::env::temp_dir().join(format!("om-untrusted-{}", std::process::id()));
    std::fs::create_dir_all(dir.join(".pi/ext")).unwrap();
    std::fs::write(
        dir.join(".pi/ext/observational-memory.json"),
        r#"{"passive": true}"#,
    )
    .unwrap();
    let cwd = dir.display().to_string();
    let branch = json!(conversation(3, 16_000));
    let ctx_with = |trusted: bool| json!({ "cwd": cwd, "hasUI": false, "flags": {}, "isProjectTrusted": trusted });

    let mut untrusted = TestHost::start(extension());
    untrusted.initialize().await;
    untrusted
        .request(50, "event", event("turn_end", ctx_with(false)))
        .await;
    untrusted
        .drive(
            |method, _| match method {
                "session.getBranch" => branch.clone(),
                _ => json!({}),
            },
            |m| m["method"] == "session.getBranch",
        )
        .await;
    untrusted.finish().await.unwrap();

    let mut trusted = TestHost::start(extension());
    trusted.initialize().await;
    trusted
        .request(51, "event", event("turn_end", ctx_with(true)))
        .await;
    trusted
        .drive(|_, _| json!({}), |m| m["method"] == "settings.get")
        .await;
    let methods = methods_until_status(&mut trusted, 52, &branch).await;
    assert_eq!(
        methods.iter().filter(|m| *m == "session.getBranch").count(),
        1,
        "trusted project file sets passive: {methods:?}"
    );
    trusted.finish().await.unwrap();
    std::fs::remove_dir_all(dir).ok();
}
