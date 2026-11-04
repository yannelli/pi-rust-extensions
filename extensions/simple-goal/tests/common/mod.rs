#![allow(dead_code)]

use std::collections::HashMap;
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
        let name = format!("pi-simple-goal-{label}-{}-{id}", std::process::id());
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

/// An in-memory pi host. `session.appendEntry` adds to `branch`, which `session.getBranch` returns.
pub struct Host {
    writer: WriteHalf<DuplexStream>,
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    next_id: u64,
    pub calls: Vec<(String, Value)>,
    /// Text of the `llm.complete` reply; `None` answers with an error.
    pub verdict: Option<String>,
    /// When set, the `typesafe` extension is loaded and answers decision requests with this
    /// probability of done.
    pub typesafe: Option<f64>,
    pub branch: Vec<Value>,
    pub ctx: Value,
    responses: HashMap<u64, Value>,
}

impl Host {
    pub async fn start(work: &Path, agent: &Path, branch: Vec<Value>) -> Self {
        let (host_side, ext_side) = tokio::io::duplex(256 * 1024);
        let (read, write) = tokio::io::split(ext_side);
        tokio::spawn(pi_ext_simple_goal::extension().serve(read, write));
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
            verdict: Some("Looks finished.\n<done/>".into()),
            typesafe: None,
            branch,
            ctx,
            responses: HashMap::new(),
        };
        let init = json!({
            "protocolVersion": 1, "piVersion": "0.8.0", "cwd": work, "agentDir": agent,
            "extensionPath": "native:test/simple-goal", "platform": "darwin", "mode": "tui",
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

    fn answer_for(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        Ok(match method {
            "session.appendEntry" => {
                self.branch.push(json!({
                    "type": "custom", "customType": params["customType"], "data": params["data"]
                }));
                Value::Null
            }
            "session.getBranch" => Value::Array(self.branch.clone()),
            "tools.getAll" if self.typesafe.is_some() => json!([{ "name": "typesafe_evaluate" }]),
            "exec" => json!({ "stdout": "", "stderr": "", "code": 0, "killed": false }),
            "llm.complete" => match &self.verdict {
                Some(text) => json!({ "message": { "role": "assistant" }, "text": text }),
                None => return Err("model unavailable".into()),
            },
            _ => Value::Null,
        })
    }

    /// Answers one extension message; returns it when it was a response.
    async fn handle(&mut self, message: Value) -> Option<Value> {
        let Some(method) = message["method"].as_str().map(String::from) else {
            return Some(message);
        };
        self.calls.push((method.clone(), message["params"].clone()));
        if message.get("id").is_some() {
            let reply = match self.answer_for(&method, &message["params"]) {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": message["id"], "result": result }),
                Err(err) => json!({ "jsonrpc": "2.0", "id": message["id"],
                    "error": { "code": -32000, "message": err } }),
            };
            self.send(reply).await;
        }
        let params = &message["params"];
        if let (Some(p), "events.emit") = (self.typesafe, method.as_str())
            && params["channel"] == "typesafe:decision:request"
        {
            let result = json!({ "model": "typesafe/jev-1.14", "answers": { "done": { "type": "noul", "noul": p } } });
            let data = json!({ "id": params["data"]["id"], "ok": true, "result": result });
            let reply = json!({ "jsonrpc": "2.0", "method": "events.message",
                "params": { "channel": "typesafe:decision:reply", "data": data } });
            self.send(reply).await;
        }
        None
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.send(request).await;
        let response = loop {
            if let Some(response) = self.responses.remove(&id) {
                break response;
            }
            let message = self.next().await;
            if let Some(response) = self.handle(message).await {
                let id = response["id"].as_u64().unwrap_or_default();
                self.responses.insert(id, response);
            }
        };
        assert!(response.get("error").is_none(), "error reply: {response}");
        response["result"].clone()
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

    pub async fn command(&mut self, name: &str, args: &str) {
        let params = json!({ "name": name, "args": args, "ctx": self.ctx });
        self.request("command.run", params).await;
    }

    /// Reads and answers messages until the extension calls `method`; returns its params.
    pub async fn wait_for(&mut self, method: &str) -> Value {
        loop {
            let message = self.next().await;
            let found = message["method"] == method;
            let params = message["params"].clone();
            if let Some(response) = self.handle(message).await {
                let id = response["id"].as_u64().unwrap_or_default();
                self.responses.insert(id, response);
            }
            if found {
                return params;
            }
        }
    }

    pub fn called(&self, method: &str) -> Vec<&Value> {
        let calls = self.calls.iter().filter(|(name, _)| name == method);
        calls.map(|(_, params)| params).collect()
    }

    /// Simulates a run that made `tool_calls` tool calls, then settled.
    pub async fn run(&mut self, tool_calls: usize, aborted: bool) {
        for _ in 0..tool_calls {
            let call =
                json!({ "type": "tool_call", "toolName": "bash", "toolCallId": "t", "input": {} });
            self.event("tool_call", call).await;
        }
        let settled = json!({ "type": "agent_settled", "aborted": aborted });
        self.event("agent_settled", settled).await;
    }
}
