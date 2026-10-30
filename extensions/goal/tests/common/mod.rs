#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

/// A temp directory removed on drop.
pub struct TestDir(PathBuf);

impl TestDir {
    pub fn new(label: &str) -> Self {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let name = format!("pi-goal-{label}-{}-{id}", std::process::id());
        let path = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Default answers of a pi host with no model, git output, or saved entries.
pub fn default_answer(method: &str, params: &Value, verdict: &str) -> Value {
    match method {
        "session.getBranch" | "session.getEntries" | "tools.getAll" | "commands.getAll" => {
            json!([])
        }
        "tools.getActive" => json!(["read", "bash", "edit", "write"]),
        "ui.confirm" | "ctx.isIdle" => json!(true),
        "ui.getEditorText" => json!(""),
        "exec" => json!({ "stdout": "", "stderr": "", "code": 0, "killed": false }),
        "llm.complete" => json!({ "message": { "role": "assistant" }, "text": verdict }),
        "pi.call" if params["method"] == "isProjectTrusted" => json!(true),
        _ => Value::Null,
    }
}

pub struct Host {
    writer: WriteHalf<DuplexStream>,
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    next_id: u64,
    pub calls: Vec<(String, Value)>,
    pub verdict: String,
    pub ctx: Value,
}

impl Host {
    pub async fn start(work: &Path, agent: &Path) -> Self {
        let (host_side, ext_side) = tokio::io::duplex(256 * 1024);
        let (read, write) = tokio::io::split(ext_side);
        tokio::spawn(pi_ext_goal::extension().serve(read, write));
        let (read, writer) = tokio::io::split(host_side);
        let ctx = json!({
            "cwd": work, "mode": "tui", "hasUI": true, "isIdle": true, "isProjectTrusted": true,
            "sessionId": "s1", "sessionFile": "/sessions/s1.jsonl", "flags": {},
            "model": { "provider": "mock", "id": "m1" }
        });
        let mut host = Self {
            writer,
            lines: BufReader::new(read).lines(),
            next_id: 0,
            calls: Vec::new(),
            verdict: "<approved/>".into(),
            ctx,
        };
        let init = json!({
            "protocolVersion": 1, "piVersion": "0.8.0", "cwd": work, "agentDir": agent,
            "extensionPath": "native:test/goal", "platform": "darwin", "mode": "tui",
            "hasUI": true, "flags": {}
        });
        let result = host.request("initialize", init).await;
        host.calls.push(("$initialize".into(), result));
        host.send(json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }))
            .await;
        let start = json!({ "type": "session_start", "reason": "startup" });
        host.event("session_start", start).await;
        host
    }

    pub fn init_result(&self) -> &Value {
        &self.calls[0].1
    }

    async fn send(&mut self, message: Value) {
        let line = format!("{message}\n");
        self.writer.write_all(line.as_bytes()).await.unwrap();
    }

    async fn next(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(20), self.lines.next_line())
            .await
            .expect("extension timed out")
            .unwrap()
            .expect("extension closed stdout");
        serde_json::from_str(&line).unwrap()
    }

    /// Answers one extension message; returns it when it was a response.
    async fn handle(&mut self, message: Value) -> Option<Value> {
        let Some(method) = message["method"].as_str().map(String::from) else {
            return Some(message);
        };
        self.calls.push((method.clone(), message["params"].clone()));
        if message.get("id").is_some() {
            let result = default_answer(&method, &message["params"], &self.verdict);
            let reply = json!({ "jsonrpc": "2.0", "id": message["id"], "result": result });
            self.send(reply).await;
        }
        None
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.send(request).await;
        loop {
            let message = self.next().await;
            if let Some(response) = self.handle(message).await
                && response["id"] == id
            {
                assert!(response.get("error").is_none(), "error reply: {response}");
                return response["result"].clone();
            }
        }
    }

    pub async fn event(&mut self, name: &str, event: Value) -> Value {
        let params = json!({ "name": name, "event": event, "ctx": self.ctx });
        self.request("event", params).await
    }

    /// Runs a tool and returns `(text, isError)`.
    pub async fn tool(&mut self, name: &str, params: Value) -> (String, bool) {
        let call = json!({ "toolCallId": format!("call-{name}-{}", self.next_id),
            "name": name, "params": params, "ctx": self.ctx });
        let result = self.request("tool.execute", call).await;
        let text = result["content"]
            .as_array()
            .map(|blocks| {
                let texts = blocks.iter().filter_map(|b| b["text"].as_str());
                texts.collect::<Vec<_>>().join("\n")
            })
            .unwrap_or_default();
        (text, result["isError"] == true)
    }

    /// Reads and answers messages until the extension calls `method`; returns its params.
    pub async fn wait_for(&mut self, method: &str) -> Value {
        loop {
            let message = self.next().await;
            let found = message["method"] == method;
            let params = message["params"].clone();
            self.handle(message).await;
            if found {
                return params;
            }
        }
    }

    pub fn called(&self, method: &str) -> Vec<&Value> {
        let calls = self.calls.iter().filter(|(name, _)| name == method);
        calls.map(|(_, params)| params).collect()
    }
}
