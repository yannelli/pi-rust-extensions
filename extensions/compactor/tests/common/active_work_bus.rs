//! Relay host for the Active Work integration tests.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use crate::common;
use pi_ext_compactor::extension;
use pi_ext_sdk::Extension;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, WriteHalf};
use tokio::sync::mpsc;

pub(super) const COMPACTOR: usize = 0;
pub(super) const GOAL: usize = 1;
pub(super) const TASKS: usize = 2;

pub(super) struct Bus {
    writers: Vec<WriteHalf<DuplexStream>>,
    rx: mpsc::UnboundedReceiver<(usize, Value)>,
    subscribers: HashMap<String, Vec<usize>>,
    responses: HashMap<(usize, u64), Value>,
    next_id: u64,
    pub(super) ctx: Value,
}

impl Bus {
    fn start(extensions: Vec<Extension>, ctx: Value) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut writers = Vec::new();
        for (index, ext) in extensions.into_iter().enumerate() {
            let (host_side, ext_side) = tokio::io::duplex(256 * 1024);
            let (read, write) = tokio::io::split(ext_side);
            tokio::spawn(ext.serve(read, write));
            let (read, writer) = tokio::io::split(host_side);
            writers.push(writer);
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(read).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = tx.send((index, serde_json::from_str(&line).unwrap()));
                }
            });
        }
        Self {
            writers,
            rx,
            subscribers: HashMap::new(),
            responses: HashMap::new(),
            next_id: 100,
            ctx,
        }
    }

    pub(super) async fn send(&mut self, to: usize, message: Value) {
        let line = format!("{message}\n");
        self.writers[to].write_all(line.as_bytes()).await.unwrap();
    }

    /// Answers a request of extension `from` as pi would; `events.emit` reaches every subscriber.
    async fn serve(&mut self, from: usize, message: Value) {
        let method = message["method"].as_str().unwrap_or_default().to_owned();
        let params = &message["params"];
        let channel = params["channel"].as_str().unwrap_or_default().to_owned();
        let result = match method.as_str() {
            "events.subscribe" => {
                self.subscribers.entry(channel).or_default().push(from);
                Value::Null
            }
            "events.emit" => {
                let data = params["data"].clone();
                for to in self.subscribers.get(&channel).cloned().unwrap_or_default() {
                    let note = json!({ "jsonrpc": "2.0", "method": "events.message", "params": { "channel": channel, "data": data } });
                    self.send(to, note).await;
                }
                Value::Null
            }
            "session.getBranch" | "session.getEntries" | "tools.getAll" | "commands.getAll" => {
                json!([])
            }
            "tools.getActive" => json!(["read", "bash"]),
            "ui.confirm" | "ctx.isIdle" => json!(true),
            "pi.call" if params["method"] == "isProjectTrusted" => json!(true),
            _ => Value::Null,
        };
        if let Some(id) = message.get("id") {
            let answer = json!({ "jsonrpc": "2.0", "id": id, "result": result });
            self.send(from, answer).await;
        }
    }

    async fn pump(&mut self, wait: Duration) -> bool {
        let Ok(Some((from, message))) = tokio::time::timeout(wait, self.rx.recv()).await else {
            return false;
        };
        if message.get("method").is_some() {
            self.serve(from, message).await;
        } else if let Some(id) = message["id"].as_u64() {
            self.responses.insert((from, id), message);
        }
        true
    }

    pub(super) async fn request(&mut self, to: usize, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(
            to,
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )
        .await;
        loop {
            if let Some(response) = self.responses.remove(&(to, id)) {
                return response;
            }
            assert!(
                self.pump(Duration::from_secs(10)).await,
                "no answer to {method}"
            );
        }
    }

    /// Serves the extensions until they are quiet for `quiet`.
    pub(super) async fn settle(&mut self, quiet: Duration) {
        while self.pump(quiet).await {}
    }

    pub(super) async fn session_start(&mut self, session: &str, reason: &str) {
        self.ctx["sessionId"] = json!(session);
        for to in [GOAL, TASKS, COMPACTOR] {
            let event = json!({ "type": "session_start", "reason": reason });
            let params = json!({ "name": "session_start", "event": event, "ctx": self.ctx });
            self.request(to, "event", params).await;
        }
        self.settle(Duration::from_millis(300)).await;
    }

    pub(super) async fn tool(&mut self, to: usize, name: &str, params: Value) -> Value {
        let params = json!({ "toolCallId": name, "name": name, "params": params, "ctx": self.ctx });
        self.request(to, "tool.execute", params).await["result"].clone()
    }

    /// Stops the `sleep` processes of the background-tasks extension.
    pub(super) async fn stop_tasks(&mut self) {
        let event = json!({ "type": "session_shutdown", "reason": "quit" });
        let params = json!({ "name": "session_shutdown", "event": event, "ctx": self.ctx });
        self.request(TASKS, "event", params).await;
    }

    /// The `[Active Work]` section of a vcc compaction, or an empty string.
    pub(super) async fn active_work(&mut self) -> String {
        let event = json!({
            "type": "session_before_compact",
            "preparation": { "firstKeptEntryId": "e11", "tokensBefore": 5000, "previousSummary": null,
                "messagesToSummarize": [], "turnPrefixMessages": [], "isSplitTurn": false,
                "fileOps": { "read": [], "written": [], "edited": [] } },
            "branchEntries": common::branch(), "customInstructions": null, "reason": "threshold", "willRetry": false
        });
        let params = json!({ "name": "session_before_compact", "event": event, "ctx": self.ctx });
        let response = self.request(COMPACTOR, "event", params).await;
        let summary = response["result"]["result"]["compaction"]["summary"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let Some(start) = summary.find("[Active Work]") else {
            return String::new();
        };
        let rest = &summary[start..];
        let end = rest[1..].find("\n[").map_or(rest.len(), |end| end + 1);
        rest[..end].to_owned()
    }
}

pub(super) async fn bus(root: &Path) -> Bus {
    let (work, agent) = (root.join("work"), root.join("agent"));
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&agent).unwrap();
    let ctx = json!({ "cwd": work, "mode": "print", "hasUI": false, "isIdle": true, "isProjectTrusted": true, "flags": {} });
    let extensions = vec![
        extension(),
        pi_ext_goal::extension(),
        pi_ext_background_tasks::extension(),
    ];
    let mut bus = Bus::start(extensions, ctx);
    for to in [COMPACTOR, GOAL, TASKS] {
        let init = json!({ "protocolVersion": 1, "piVersion": "0.8.0", "cwd": work, "agentDir": agent,
            "extensionPath": "native:test/test", "platform": "darwin", "mode": "print", "hasUI": false, "flags": {} });
        bus.request(to, "initialize", init).await;
        bus.send(
            to,
            json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        )
        .await;
    }
    bus
}
