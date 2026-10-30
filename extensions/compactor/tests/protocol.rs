mod common;

use common::{TestHost, branch, ctx};
use pi_ext_compactor::extension;
use serde_json::{Value, json};

fn names(list: &Value, key: &str) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|item| item[key].as_str().unwrap_or_default().to_owned())
        .collect()
}

fn project_with(config: Value) -> String {
    let dir = std::env::temp_dir().join(format!(
        "compactor-proto-{}-{}",
        std::process::id(),
        config.to_string().len()
    ));
    std::fs::create_dir_all(dir.join(".pi/ext")).unwrap();
    std::fs::write(dir.join(".pi/ext/compactor.json"), config.to_string()).unwrap();
    dir.display().to_string()
}

fn compact_event(reason: &str, instructions: Option<&str>) -> Value {
    json!({
        "type": "session_before_compact",
        "preparation": {
            "firstKeptEntryId": "e11", "tokensBefore": 5000, "previousSummary": null,
            "messagesToSummarize": [{ "role": "user", "content": "Build a CLI tool" }],
            "turnPrefixMessages": [], "isSplitTurn": false,
            "fileOps": { "read": ["/work/a.rs"], "written": [], "edited": ["/work/src/main.rs"] }
        },
        "branchEntries": branch(),
        "customInstructions": instructions,
        "reason": reason,
        "willRetry": false
    })
}

#[tokio::test]
async fn initialize_lists_tools_commands_events() {
    let mut host = TestHost::start(extension());
    let init = host.initialize().await;
    assert_eq!(
        names(&init["tools"], "name"),
        ["session_recall", "context_budget"]
    );
    let commands = names(&init["commands"], "name");
    for command in [
        "unipi:compact-vcc",
        "unipi:compact-by-llm",
        "unipi:session-recall",
        "unipi:compact-stats",
        "unipi:compact-doctor",
        "unipi:compact-help",
    ] {
        assert!(commands.contains(&command.to_owned()), "{commands:?}");
    }
    let events: Vec<&str> = init["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for event in [
        "session_start",
        "context",
        "input",
        "session_before_compact",
        "session_compact",
        "turn_end",
        "agent_end",
        "agent_settled",
    ] {
        assert!(events.contains(&event), "{events:?}");
    }
    assert_eq!(init["tools"][0]["label"], "Session Recall");
    host.finish().await.unwrap();
}

#[tokio::test]
async fn before_compact_returns_vcc_summary() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let params = json!({ "name": "session_before_compact", "event": compact_event("threshold", None), "ctx": ctx() });
    host.request(5, "event", params).await;
    let response = host.expect_response(5).await;
    let compaction = &response["result"]["result"]["compaction"];
    assert_eq!(compaction["firstKeptEntryId"], "e8");
    assert_eq!(compaction["tokensBefore"], 5000);
    assert_eq!(compaction["details"]["method"], "vcc");
    let summary = compaction["summary"].as_str().unwrap();
    assert!(
        summary.contains("[Your Requests]") && summary.contains("[Files]"),
        "{summary}"
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn manual_compact_follows_pi_compact_llm() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let cwd = project_with(json!({ "piCompact": "llm", "llmInstructions": "Focus on tests." }));
    let ctx = json!({ "cwd": cwd, "hasUI": true, "flags": {} });
    let params = json!({ "name": "session_before_compact", "event": compact_event("manual", None), "ctx": ctx });
    host.request(6, "event", params).await;
    let (id, request) = host.expect_request("llm.complete").await;
    assert!(
        request["systemPrompt"]
            .as_str()
            .unwrap()
            .starts_with("You are a context summarization assistant.")
    );
    let prompt = request["prompt"].as_str().unwrap();
    assert!(
        prompt.contains("[User]: Build a CLI tool")
            && prompt.ends_with("Additional focus: Focus on tests."),
        "{prompt}"
    );
    host.answer(
        &id,
        json!({ "text": "## Goal\nCLI", "message": { "usage": { "input": 10 } } }),
    )
    .await;
    let response = host.expect_response(6).await;
    let compaction = &response["result"]["result"]["compaction"];
    assert_eq!(compaction["details"]["method"], "llm");
    assert_eq!(compaction["firstKeptEntryId"], "e11");
    let summary = compaction["summary"].as_str().unwrap();
    assert!(
        summary.starts_with("## Goal\nCLI\n\n<read-files>\n/work/a.rs"),
        "{summary}"
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn compact_vcc_command_wins_over_config_method() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let cwd = project_with(json!({ "method": "llm" }));
    let ctx = json!({ "cwd": cwd, "hasUI": true, "flags": {} });
    host.request(
        7,
        "command.run",
        json!({ "name": "unipi:compact-vcc", "args": "keep:2", "ctx": ctx }),
    )
    .await;
    let mut compact_id = None;
    for _ in 0..2 {
        let message = host.recv().await;
        if message["method"] == "ctx.compact" {
            assert_eq!(
                message["params"]["customInstructions"],
                "__compactor__ keep:2"
            );
            compact_id = Some(message["id"].clone());
        } else {
            assert_eq!(message["id"], 7, "{message}");
        }
    }
    let event = compact_event("manual", Some("__compactor__ keep:2"));
    host.request(
        8,
        "event",
        json!({ "name": "session_before_compact", "event": event, "ctx": ctx }),
    )
    .await;
    let response = host.expect_response(8).await;
    let compaction = &response["result"]["result"]["compaction"];
    assert_eq!(compaction["details"]["method"], "vcc");
    assert_eq!(compaction["firstKeptEntryId"], "e8");
    host.answer(&compact_id.unwrap(), json!({})).await;
    host.finish().await.unwrap();
}

#[tokio::test]
async fn context_budget_tool_reports_usage() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let ctx = json!({ "cwd": "/work", "hasUI": true, "flags": {}, "contextUsage": { "tokens": 50000, "contextWindow": 200000, "percent": 25 } });
    host.request(
        9,
        "tool.execute",
        json!({ "toolCallId": "t1", "name": "context_budget", "params": {}, "ctx": ctx }),
    )
    .await;
    let response = host.expect_response(9).await;
    assert_eq!(
        response["result"]["content"][0]["text"],
        "Context: ~25% full (estimated 150,000 tokens remaining)\nAdvice: Plenty of room. Pi compacts automatically near the limit."
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn compact_then_is_handled_from_input() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let event = json!({ "type": "input", "text": "/unipi:compact-then run the tests", "source": "interactive" });
    host.request(
        10,
        "event",
        json!({ "name": "input", "event": event, "ctx": ctx() }),
    )
    .await;
    let mut compact_id = None;
    for _ in 0..2 {
        let message = host.recv().await;
        if message["method"] == "ctx.compact" {
            assert_eq!(message["params"]["customInstructions"], "__compactor__");
            compact_id = Some(message["id"].clone());
        } else {
            assert_eq!(
                message["result"]["result"],
                json!({ "action": "handled" }),
                "{message}"
            );
        }
    }
    host.answer(&compact_id.unwrap(), json!({})).await;
    let (id, params) = host.expect_request("session.sendUserMessage").await;
    assert_eq!(
        params,
        json!({ "content": "run the tests", "deliverAs": "followUp" })
    );
    host.answer(&id, Value::Null).await;
    host.finish().await.unwrap();
}

#[tokio::test]
async fn context_drops_legacy_custom_messages() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let messages = json!([
        { "role": "user", "content": "hi" },
        { "role": "custom", "customType": "compactor-auto-continue", "content": "x" }
    ]);
    let event = json!({ "type": "context", "messages": messages });
    host.request(
        11,
        "event",
        json!({ "name": "context", "event": event, "ctx": ctx() }),
    )
    .await;
    let response = host.expect_response(11).await;
    assert_eq!(
        response["result"]["result"],
        json!({ "messages": [{ "role": "user", "content": "hi" }] })
    );
    host.finish().await.unwrap();
}

/// Answers extension requests with `null` and returns every message up to the response to `id`.
async fn until_response(host: &mut TestHost, id: u64) -> Vec<Value> {
    let mut seen = Vec::new();
    loop {
        let message = host.recv().await;
        if let (Some(_), Some(request)) = (message["method"].as_str(), message.get("id")) {
            host.answer(&request.clone(), Value::Null).await;
        }
        let done = message["id"] == id && message.get("method").is_none();
        seen.push(message);
        if done {
            return seen;
        }
    }
}

fn notices(seen: &[Value]) -> Vec<String> {
    seen.iter()
        .filter(|m| m["method"] == "ui.notify")
        .map(|m| {
            m["params"]["message"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

#[tokio::test]
async fn overflow_compaction_falls_through_to_pi() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let params = json!({ "name": "session_before_compact", "event": compact_event("overflow", None), "ctx": ctx() });
    host.request(60, "event", params).await;
    assert_eq!(host.expect_response(60).await["result"], json!({}));
    let marked = json!({ "name": "session_before_compact",
        "event": compact_event("overflow", Some("__compactor__")), "ctx": ctx() });
    host.request(61, "event", marked).await;
    assert_eq!(host.expect_response(61).await["result"], json!({}));
    host.finish().await.unwrap();
}

#[tokio::test]
async fn non_matching_input_is_not_handled() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    for (id, text) in [
        (62, "run the tests"),
        (63, "/unipi:compact-thenx"),
        (64, "please /unipi:compact-then"),
    ] {
        let event = json!({ "type": "input", "text": text, "source": "interactive" });
        host.request(
            id,
            "event",
            json!({ "name": "input", "event": event, "ctx": ctx() }),
        )
        .await;
        assert_eq!(
            host.expect_response(id).await["result"],
            json!({}),
            "{text}"
        );
    }
    host.finish().await.unwrap();
}

#[tokio::test]
async fn steered_compact_then_is_cancelled_when_the_run_settles() {
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let event = json!({ "type": "input", "text": "/unipi:compact-then go on",
        "source": "interactive", "streamingBehavior": "steer" });
    host.request(
        65,
        "event",
        json!({ "name": "input", "event": event, "ctx": ctx() }),
    )
    .await;
    let queued = until_response(&mut host, 65).await;
    assert!(notices(&queued)[0].starts_with("Compact then: queued"));

    let settled =
        json!({ "name": "agent_settled", "event": { "type": "agent_settled" }, "ctx": ctx() });
    host.request(66, "event", settled.clone()).await;
    let seen = until_response(&mut host, 66).await;
    assert_eq!(
        notices(&seen),
        ["Compact then: cancelled — the run ended before the next turn."]
    );
    assert_eq!(seen.last().unwrap()["result"], json!({}));

    host.request(67, "event", settled).await;
    let again = until_response(&mut host, 67).await;
    assert!(notices(&again).is_empty(), "{again:?}");
    host.finish().await.unwrap();
}
