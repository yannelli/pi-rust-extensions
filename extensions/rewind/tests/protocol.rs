use std::path::Path;
use std::time::Duration;

use pi_ext_rewind::testdir::TestDir;
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};

struct Host {
    writer: WriteHalf<DuplexStream>,
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    next_id: u64,
    calls: Vec<(String, Value)>,
}

impl Host {
    fn start() -> Self {
        let (host_side, ext_side) = tokio::io::duplex(256 * 1024);
        let (read, write) = tokio::io::split(ext_side);
        tokio::spawn(pi_ext_rewind::extension().serve(read, write));
        let (read, writer) = tokio::io::split(host_side);
        Self {
            writer,
            lines: BufReader::new(read).lines(),
            next_id: 0,
            calls: Vec::new(),
        }
    }

    async fn send(&mut self, message: Value) {
        let line = format!("{message}\n");
        self.writer.write_all(line.as_bytes()).await.unwrap();
    }

    /// Sends a request and answers the extension's requests with `answer` until the response.
    async fn request(
        &mut self,
        method: &str,
        params: Value,
        answer: &mut impl FnMut(&str, &Value) -> Value,
    ) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        loop {
            let line = tokio::time::timeout(Duration::from_secs(20), self.lines.next_line())
                .await
                .expect("extension timed out")
                .unwrap()
                .expect("extension closed stdout");
            let message: Value = serde_json::from_str(&line).unwrap();
            if let Some(name) = message["method"].as_str() {
                self.calls
                    .push((name.to_string(), message["params"].clone()));
                if message.get("id").is_some() {
                    let result = answer(name, &message["params"]);
                    self.send(json!({ "jsonrpc": "2.0", "id": message["id"], "result": result }))
                        .await;
                }
            } else if message["id"] == id {
                assert!(message.get("error").is_none(), "error reply: {message}");
                return message["result"].clone();
            }
        }
    }

    async fn event(
        &mut self,
        name: &str,
        event: Value,
        ctx: &Value,
        answer: &mut impl FnMut(&str, &Value) -> Value,
    ) {
        self.request(
            "event",
            json!({ "name": name, "event": event, "ctx": ctx }),
            answer,
        )
        .await;
    }

    fn called(&self, method: &str) -> Vec<&Value> {
        self.calls
            .iter()
            .filter(|(name, _)| name == method)
            .map(|(_, params)| params)
            .collect()
    }
}

fn user(id: &str, text: &str) -> Value {
    json!({ "type": "message", "id": id, "message": { "role": "user", "content": text } })
}

fn checkpoint_entry(data: &Value) -> Value {
    json!({ "type": "custom", "id": "cp1", "customType": "pi-checkpoint", "data": data })
}

async fn start(host: &mut Host, work: &Path, agent: &Path, trusted: bool) -> Value {
    let init = json!({
        "protocolVersion": 1, "piVersion": "0.8.0", "cwd": work, "agentDir": agent,
        "extensionPath": "native:test/rewind", "platform": "darwin", "mode": "tui", "hasUI": true, "flags": {}
    });
    let result = host
        .request("initialize", init, &mut |_, _| Value::Null)
        .await;
    host.send(json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }))
        .await;
    let ctx = json!({ "cwd": work, "hasUI": true, "sessionFile": "/sessions/s1.jsonl", "flags": {},
        "isProjectTrusted": trusted });
    host.event(
        "session_start",
        json!({ "type": "session_start", "reason": "startup" }),
        &ctx,
        &mut |method, _| match method {
            "session.getBranch" => json!([]),
            _ => Value::Null,
        },
    )
    .await;
    assert!(result["commands"].to_string().contains("rewind"));
    for event in [
        "session_start",
        "message_start",
        "agent_end",
        "session_before_tree",
        "session_tree",
        "session_compact",
    ] {
        assert!(
            result["events"].to_string().contains(event),
            "missing {event}"
        );
    }
    ctx
}

/// Runs one turn that edits `a.txt` and returns the appended checkpoint data.
async fn turn(host: &mut Host, work: &Path, ctx: &Value) -> Value {
    let branch = json!([user("u1", "edit a")]);
    let message = json!({ "type": "message_start", "message": { "role": "assistant" } });
    host.event(
        "message_start",
        message,
        ctx,
        &mut |method, _| match method {
            "session.getBranch" => branch.clone(),
            _ => Value::Null,
        },
    )
    .await;
    std::fs::write(work.join("a.txt"), "two\n").unwrap();
    std::fs::write(work.join("b.txt"), "new\n").unwrap();
    host.event(
        "agent_end",
        json!({ "type": "agent_end" }),
        ctx,
        &mut |_, _| Value::Null,
    )
    .await;
    let appended = host.called("session.appendEntry");
    assert_eq!(appended.len(), 1);
    assert_eq!(appended[0]["customType"], "pi-checkpoint");
    appended[0]["data"].clone()
}

#[tokio::test]
async fn rewind_restores_code_and_conversation() {
    let work = TestDir::new("proto-work");
    let agent = TestDir::new("proto-agent");
    std::fs::write(work.path().join("a.txt"), "one\n").unwrap();
    let mut host = Host::start();
    let ctx = start(&mut host, work.path(), agent.path(), false).await;
    let data = turn(&mut host, work.path(), &ctx).await;
    assert_eq!(data["userEntryId"], "u1");
    assert_eq!(data["fileCount"], 2);
    assert_ne!(data["beforeCommit"], data["afterCommit"]);
    assert!(
        agent
            .path()
            .join("ext/rewind/sessions/s1/.git/HEAD")
            .is_file()
    );

    let branch = json!([user("u1", "edit a"), checkpoint_entry(&data)]);
    let mut navigated = Vec::new();
    let command = json!({ "name": "rewind", "args": "", "ctx": ctx });
    host.request("command.run", command, &mut |method, params| match method {
        "session.getBranch" => branch.clone(),
        "ui.select" if params["title"] == "Rewind to checkpoint:" => {
            assert_eq!(params["options"][1], "(current)");
            params["options"][0].clone()
        }
        "ui.select" => {
            assert_eq!(params["options"].as_array().unwrap().len(), 3);
            json!("Restore code and conversation")
        }
        "pi.call" => {
            navigated.push(params.clone());
            json!({ "editorText": "edit a", "cancelled": false })
        }
        _ => Value::Null,
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).unwrap(),
        "one\n"
    );
    assert!(!work.path().join("b.txt").exists());
    assert_eq!(navigated.len(), 1);
    assert_eq!(navigated[0]["target"], "ctx");
    assert_eq!(navigated[0]["method"], "navigateTree");
    assert_eq!(navigated[0]["args"], json!(["u1", { "summarize": false }]));
    assert_eq!(host.called("ui.setEditorText")[0]["text"], "edit a");
    let notes = host.called("ui.notify");
    assert!(
        notes.last().unwrap()["message"]
            .as_str()
            .unwrap()
            .starts_with("Restored code and conversation")
    );
}

#[tokio::test]
async fn tree_navigation_syncs_files_when_always() {
    let work = TestDir::new("tree-work");
    let agent = TestDir::new("tree-agent");
    std::fs::write(work.path().join("a.txt"), "one\n").unwrap();
    std::fs::create_dir_all(work.path().join(".pi/ext")).unwrap();
    let config = json!({ "rewind": { "restoreOnTree": "always" } });
    std::fs::write(work.path().join(".pi/ext/rewind.json"), config.to_string()).unwrap();
    let mut host = Host::start();
    let ctx = start(&mut host, work.path(), agent.path(), true).await;
    let data = turn(&mut host, work.path(), &ctx).await;

    let entries = json!([user("u1", "edit a"), checkpoint_entry(&data)]);
    let before = json!({ "type": "session_before_tree", "preparation": { "targetId": "u1", "userWantsSummary": false } });
    host.event("session_before_tree", before, &ctx, &mut |_, _| Value::Null)
        .await;
    assert!(host.called("ui.confirm").is_empty());
    let after = json!({ "type": "session_tree", "newLeafId": null, "oldLeafId": "cp1" });
    host.event("session_tree", after, &ctx, &mut |method, _| match method {
        "session.getEntries" => entries.clone(),
        "session.getBranch" => json!([]),
        _ => Value::Null,
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(work.path().join("a.txt")).unwrap(),
        "one\n"
    );
    assert!(!work.path().join("b.txt").exists());
}
