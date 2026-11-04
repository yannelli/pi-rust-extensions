use pi_ext_sdk::test_util::TestHost;
use pi_ext_tool_results::extension;
use pi_ext_tool_results::lines::short;
use serde_json::{Value, json};

fn ctx(cwd: &str) -> Value {
    json!({ "cwd": cwd, "hasUI": false, "flags": {}, "sessionId": "s-1", "isProjectTrusted": false })
}

/// Answers the host requests the extension makes: trust checks and notifications.
fn fake(method: &str, _params: &Value) -> Value {
    match method {
        "pi.call" => json!(false),
        _ => Value::Null,
    }
}

struct Run {
    host: TestHost,
    root: String,
    next: u64,
    /// The `ui.notify` messages the extension sent.
    notes: Vec<String>,
}

impl Run {
    async fn start(dir: &tempfile::TempDir) -> Self {
        let root = dir.path().display().to_string();
        let mut host = TestHost::start(extension()).cwd(&root).agent_dir(&root);
        let init = host.initialize().await;
        assert_eq!(
            init["events"],
            json!(["session_start", "session_shutdown", "tool_result"])
        );
        assert_eq!(init["tools"][0]["name"], "tool_result_get");
        assert_eq!(init["commands"][0]["name"], "tool-results");
        let mut run = Self {
            host,
            root,
            next: 1,
            notes: Vec::new(),
        };
        let start = json!({ "type": "session_start", "reason": "startup" });
        assert_eq!(run.event("session_start", start).await, json!({}));
        run
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let mut notes = Vec::new();
        let reply = self
            .host
            .drive(self.next, method, params, |method, params| {
                if method == "ui.notify" {
                    notes.push(params["message"].as_str().unwrap_or_default().to_owned());
                }
                fake(method, params)
            })
            .await;
        self.notes.extend(notes);
        reply["result"].clone()
    }

    async fn event(&mut self, name: &str, event: Value) -> Value {
        let params = json!({ "name": name, "event": event, "ctx": ctx(&self.root) });
        self.call("event", params).await
    }

    async fn result(&mut self, tool: &str, id: &str, content: Value, is_error: bool) -> Value {
        let event = json!({ "type": "tool_result", "toolName": tool, "toolCallId": id,
                            "input": {}, "content": content, "isError": is_error });
        self.event("tool_result", event).await
    }

    async fn get(&mut self, args: Value) -> Value {
        let params = json!({ "toolCallId": "g", "name": "tool_result_get", "params": args, "ctx": ctx(&self.root) });
        self.call("tool.execute", params).await
    }
}

fn text(reply: &Value) -> &str {
    reply["patch"]["content"][0]["text"].as_str().unwrap()
}

#[tokio::test]
async fn cleans_results_and_leaves_others() {
    let dir = tempfile::tempdir().unwrap();
    let mut run = Run::start(&dir).await;
    let noisy = json!([{ "type": "text", "text": "\u{1b}[31merror\u{1b}[0m   \n\n\n\nnext" }]);
    let reply = run.result("bash", "c1", noisy.clone(), false).await;
    assert_eq!(text(&reply), "error\n\nnext");
    let reply = run.result("bash", "c2", noisy.clone(), true).await;
    assert_eq!(text(&reply), "error\n\nnext", "errors get the cleanup");
    assert_eq!(
        run.result("tool_result_get", "c3", noisy, false).await,
        json!({})
    );
    let image = json!([{ "type": "image", "data": "AAAA", "mimeType": "image/png" }]);
    assert_eq!(run.result("bash", "c4", image, false).await, json!({}));
    let json_text = serde_json::to_string_pretty(
        &json!({ "items": [{ "id": 1, "n": "a" }, { "id": 2, "n": "b" }] }),
    )
    .unwrap();
    let reply = run
        .result(
            "mcp__x__list",
            "c5",
            json!([{ "type": "text", "text": json_text }]),
            false,
        )
        .await;
    assert_eq!(text(&reply), "items[2]{id,n}:\n  1,a\n  2,b");
}

#[tokio::test]
async fn stores_large_results_and_reads_them_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut run = Run::start(&dir).await;
    let big: String = (1..=3000)
        .map(|n| format!("entry {n} status=ok\n"))
        .collect();
    let reply = run
        .result(
            "bash",
            "call-big",
            json!([{ "type": "text", "text": big }]),
            false,
        )
        .await;
    let shown = text(&reply);
    assert!(shown.starts_with("entry 1 status=ok\n"), "{shown}");
    let id = shown
        .split("result_id \"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    let notice = format!(
        "Result truncated ({} characters, 3,000 lines).",
        short(big.chars().count())
    );
    assert!(shown.contains(&notice), "{shown}");
    let session_dir = dir.path().join("ext/tool-results/sessions/s-1");
    assert!(session_dir.join(format!("{id}.txt")).is_file());

    let lines = run
        .get(json!({ "result_id": id, "mode": "lines", "start": 1500, "end": 1501 }))
        .await;
    let expected = format!(
        "Result {id}: lines 1500-1501 of 3,000\n1500\tentry 1500 status=ok\n1501\tentry 1501 status=ok"
    );
    assert_eq!(lines["content"][0]["text"], expected);
    let found = run.get(json!({ "result_id": id, "mode": "search", "pattern": "entry 2999 ", "context_chars": 5 })).await;
    let found = found["content"][0]["text"].as_str().unwrap();
    assert!(
        found.contains("--- line 2999\n…s=ok\nentry 2999 statu…"),
        "{found}"
    );
    let full = run.get(json!({ "result_id": id, "mode": "full" })).await;
    assert_eq!(
        full["content"][0]["text"].as_str().unwrap().len(),
        big.len()
    );
    let missing = run.get(json!({ "result_id": "tr-none" })).await;
    assert_eq!(missing["isError"], true);
    let bad = run.get(json!({ "result_id": id, "mode": "search" })).await;
    assert_eq!(bad["isError"], true);

    let stop = json!({ "type": "session_shutdown", "reason": "quit" });
    run.event("session_shutdown", stop).await;
    assert!(
        !session_dir.exists(),
        "the store is removed with the session"
    );
    let stats = std::fs::read_to_string(dir.path().join("ext/tool-results/stats.json")).unwrap();
    assert!(stats.contains("\"truncated\": 1"), "{stats}");
}

#[tokio::test]
async fn rules_and_dry_run_from_the_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let config = json!({ "dryRun": false, "tools": { "bash": { "enabled": false }, "mcp__*": { "json": "compact" } } });
    std::fs::create_dir_all(dir.path().join("ext")).unwrap();
    std::fs::write(dir.path().join("ext/tool-results.json"), config.to_string()).unwrap();
    let mut run = Run::start(&dir).await;
    let noisy = json!([{ "type": "text", "text": "a   \n\n\n\nb" }]);
    assert_eq!(
        run.result("bash", "c1", noisy.clone(), false).await,
        json!({})
    );
    let pretty = json!([{ "type": "text", "text": "{\n  \"a\": [1, 2]\n}" }]);
    assert_eq!(
        text(&run.result("mcp__s__t", "c2", pretty, false).await),
        "{\"a\":[1,2]}"
    );

    std::fs::write(
        dir.path().join("ext/tool-results.json"),
        json!({ "dryRun": true }).to_string(),
    )
    .unwrap();
    let start = json!({ "type": "session_start", "reason": "reload" });
    run.event("session_start", start).await;
    assert_eq!(run.result("grep", "c3", noisy, false).await, json!({}));
    let params = json!({ "name": "tool-results", "args": "stats", "ctx": ctx(&run.root) });
    run.call("command.run", params).await;
    let stats = run.notes.last().unwrap();
    assert!(stats.contains("mcp__s__t: "), "{stats}");
    assert!(stats.contains("dry run would save: "), "{stats}");
    let params = json!({ "name": "tool-results", "args": "", "ctx": ctx(&run.root) });
    run.call("command.run", params).await;
    assert!(
        run.notes.last().unwrap().contains("tool-results (dry run"),
        "{:?}",
        run.notes
    );
}
