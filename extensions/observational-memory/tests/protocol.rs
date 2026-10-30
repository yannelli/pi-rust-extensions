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
    let mut host = TestHost::start(extension());
    host.initialize().await;
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
    assert_eq!(host.expect_response(10).await["result"], Value::Null);
    host.finish().await.unwrap();
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
