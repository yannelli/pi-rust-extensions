#![allow(dead_code)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use pi_ext_sdk::Extension;
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};
use tokio::task::JoinHandle;

/// A scripted pi host that answers every extension request with `null` unless told otherwise.
pub struct TestHost {
    writer: Option<WriteHalf<DuplexStream>>,
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    server: JoinHandle<pi_ext_sdk::Result<()>>,
    /// Every message the extension sent, in order.
    pub seen: Vec<Value>,
    pending_custom: Option<Value>,
}

impl TestHost {
    pub fn start(ext: Extension) -> Self {
        let (host_side, ext_side) = tokio::io::duplex(256 * 1024);
        let (ext_read, ext_write) = tokio::io::split(ext_side);
        let server = tokio::spawn(ext.serve(ext_read, ext_write));
        let (read, write) = tokio::io::split(host_side);
        Self {
            writer: Some(write),
            lines: BufReader::new(read).lines(),
            server,
            seen: Vec::new(),
            pending_custom: None,
        }
    }

    pub async fn send(&mut self, message: Value) {
        let writer = self.writer.as_mut().expect("writer open");
        writer
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
    }

    pub async fn request(&mut self, id: u64, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
    }

    pub async fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await;
    }

    async fn recv(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(15), self.lines.next_line())
            .await
            .expect("timed out waiting for the extension")
            .unwrap()
            .expect("extension closed stdout");
        serde_json::from_str(&line).unwrap()
    }

    /// Answers requests with `null` until the response to `id` arrives; returns it.
    pub async fn until_response(&mut self, id: u64) -> Value {
        loop {
            let message = self.recv().await;
            if message.get("method").is_none() && message["id"] == id {
                return message;
            }
            self.auto_answer(&message).await;
        }
    }

    /// Answers requests with `null` until a message with `method` arrives; returns its params.
    pub async fn until_method(&mut self, method: &str) -> Value {
        loop {
            let message = self.recv().await;
            let hit = message["method"] == method;
            self.auto_answer(&message).await;
            if hit {
                return message["params"].clone();
            }
        }
    }

    async fn auto_answer(&mut self, message: &Value) {
        self.seen.push(message.clone());
        if message["method"] == "ui.custom" {
            self.pending_custom = message.get("id").cloned();
            return;
        }
        if message["method"] == "ui.close"
            && let Some(id) = self.pending_custom.take()
        {
            let result = json!({ "result": message["params"]["result"] });
            self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
                .await;
            return;
        }
        let id = message
            .get("id")
            .filter(|_| message.get("method").is_some());
        if let Some(id) = id.cloned() {
            self.send(json!({ "jsonrpc": "2.0", "id": id, "result": null }))
                .await;
        }
    }

    pub fn seen_of(&self, method: &str) -> Vec<Value> {
        let hits = self.seen.iter().filter(|m| m["method"] == method);
        hits.map(|m| m["params"].clone()).collect()
    }

    pub async fn initialize(&mut self, agent_dir: &Path, cwd: &Path) -> Value {
        let params = json!({
            "protocolVersion": 1, "piVersion": "0.8.0", "cwd": cwd, "agentDir": agent_dir,
            "extensionPath": "native:test/subagents", "platform": "darwin", "mode": "tui",
            "hasUI": true, "flags": {}
        });
        self.request(0, "initialize", params).await;
        let response = self.until_response(0).await;
        self.notify("initialized", json!({})).await;
        response["result"].clone()
    }

    pub async fn finish(mut self) {
        if let Some(mut writer) = self.writer.take() {
            writer.shutdown().await.unwrap();
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), self.server).await;
    }
}

pub fn ctx(cwd: &Path) -> Value {
    json!({
        "cwd": cwd, "hasUI": true, "flags": {}, "isProjectTrusted": true, "thinkingLevel": "high",
        "model": { "provider": "mock", "id": "parent-model" }
    })
}

/// A fake `pi` that records its args, env, and pid in `dir`, prints `events`, then
/// sleeps `hang_secs` when it is above 0.
pub fn fake_pi(dir: &Path, events: &[Value], hang_secs: u32) -> PathBuf {
    let stream: Vec<String> = events.iter().map(Value::to_string).collect();
    std::fs::write(dir.join("events.jsonl"), stream.join("\n") + "\n").unwrap();
    let d = dir.display();
    let script = format!(
        "#!/bin/sh\necho $$ > '{d}/pid'\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{d}/args'\n\
env | grep '^PI_' > '{d}/env'\npwd > '{d}/cwd'\nwhile IFS= read -r line; do printf '%s\\n' \"$line\"; sleep 0.04; done < '{d}/events.jsonl'\n\
if [ {hang_secs} -gt 0 ]; then sleep {hang_secs}; fi\n"
    );
    let path = dir.join("fake-pi");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// A child event stream: one read call, one todo call, and a final answer.
pub fn scripted_events(answer: &str) -> Vec<Value> {
    vec![
        json!({"type": "session", "version": 3, "id": "child", "cwd": "/w"}),
        json!({"type": "agent_start"}),
        json!({"type": "turn_start"}),
        json!({"type": "tool_execution_start", "toolCallId": "t1", "toolName": "read", "args": {"path": "src/main.rs"}}),
        json!({"type": "tool_execution_end", "toolCallId": "t1", "toolName": "read", "result": {"content": []}, "isError": false}),
        json!({"type": "tool_execution_start", "toolCallId": "t2", "toolName": "todo", "args": {"action": "create"}}),
        json!({"type": "tool_execution_end", "toolCallId": "t2", "toolName": "todo", "isError": false,
            "result": {"content": [], "details": {"action": "create", "tasks": [{"id": 1, "subject": "scan", "status": "pending"}], "nextId": 2}}}),
        json!({"type": "message_start", "message": {"role": "assistant", "content": []}}),
        json!({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "contentIndex": 0, "delta": "Working"}}),
        json!({"type": "message_end", "message": {"role": "assistant", "content": [{"type": "text", "text": answer}],
            "stopReason": "stop", "provider": "mock", "model": "parent-model",
            "usage": {"input": 10, "output": 5, "cost": {"total": 0.001}}}}),
        json!({"type": "agent_end", "messages": []}),
        json!({"type": "agent_settled", "aborted": false}),
    ]
}

pub struct Dirs {
    pub root: pi_ext_e2e::TempDir,
    pub agent: PathBuf,
    pub cwd: PathBuf,
    pub fake: PathBuf,
}

/// Temp agent dir, cwd, and fake-pi dir; `subagents.json` points `piBin` at the fake.
pub fn dirs(events: &[Value], hang_secs: u32) -> Dirs {
    let root = pi_ext_e2e::TempDir::new().unwrap();
    let agent = root.path().join("agent");
    let cwd = root.path().join("work");
    let fake = root.path().join("fake");
    for dir in [&agent, &cwd, &fake] {
        std::fs::create_dir_all(dir.join("ext")).unwrap();
    }
    let bin = fake_pi(&fake, events, hang_secs);
    let config = json!({ "piBin": bin });
    std::fs::write(agent.join("ext/subagents.json"), config.to_string()).unwrap();
    Dirs {
        root,
        agent,
        cwd,
        fake,
    }
}
