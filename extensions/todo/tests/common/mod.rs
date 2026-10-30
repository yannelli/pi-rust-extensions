#![allow(dead_code)]

use std::time::Duration;

use pi_ext_sdk::Extension;
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};
use tokio::task::JoinHandle;

/// Methods the host answers on its own while the test waits for something else.
const AUTO: &[&str] = &[
    "session.getBranch",
    "session.appendEntry",
    "events.subscribe",
    "ui.setWidget",
    "ui.notify",
];

/// A scripted pi host over an in-memory duplex; it records every message from the extension.
pub struct TestHost {
    writer: Option<WriteHalf<DuplexStream>>,
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    server: JoinHandle<pi_ext_sdk::Result<()>>,
    pub seen: Vec<Value>,
    pub branch: Vec<Value>,
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
            branch: Vec::new(),
        }
    }

    pub async fn send(&mut self, message: Value) {
        let line = format!("{message}\n");
        self.writer
            .as_mut()
            .unwrap()
            .write_all(line.as_bytes())
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

    pub async fn answer(&mut self, id: &Value, result: Value) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
            .await;
    }

    async fn recv(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(5), self.lines.next_line())
            .await
            .expect("timed out waiting for the extension")
            .unwrap()
            .expect("extension closed stdout");
        serde_json::from_str(&line).unwrap()
    }

    /// Answers `AUTO` requests until a message matches `want`, and returns it.
    pub async fn until(&mut self, want: impl Fn(&Value) -> bool) -> Value {
        loop {
            let message = self.recv().await;
            self.seen.push(message.clone());
            if want(&message) {
                return message;
            }
            let method = message["method"].as_str().unwrap_or_default().to_owned();
            if message.get("id").is_some() && AUTO.contains(&method.as_str()) {
                let result = if method == "session.getBranch" {
                    json!(self.branch)
                } else {
                    Value::Null
                };
                if method == "session.appendEntry" {
                    let p = &message["params"];
                    self.branch.push(json!({ "type": "custom", "customType": p["customType"], "data": p["data"] }));
                }
                self.answer(&message["id"], result).await;
            }
        }
    }

    pub async fn response(&mut self, id: u64) -> Value {
        self.until(|m| m["id"] == id && m.get("method").is_none())
            .await
    }

    pub async fn method(&mut self, method: &str) -> Value {
        self.until(|m| m["method"] == method).await
    }

    pub fn seen_method(&self, method: &str) -> Vec<&Value> {
        self.seen
            .iter()
            .filter(|m| m["method"] == method)
            .map(|m| &m["params"])
            .collect()
    }

    pub async fn initialize(&mut self) -> Value {
        self.initialize_in("/nonexistent/agent").await
    }

    pub async fn initialize_in(&mut self, agent_dir: &str) -> Value {
        let params = json!({
            "protocolVersion": 1, "piVersion": "0.8.0", "cwd": "/work",
            "agentDir": agent_dir, "extensionPath": "native:test/todo",
            "platform": "darwin", "mode": "tui", "hasUI": true, "flags": {}
        });
        self.request(0, "initialize", params).await;
        let response = self.response(0).await;
        self.notify("initialized", json!({})).await;
        response["result"].clone()
    }

    pub async fn finish(mut self) {
        if let Some(mut writer) = self.writer.take() {
            writer.shutdown().await.unwrap();
        }
        let result = tokio::time::timeout(Duration::from_secs(5), self.server)
            .await
            .expect("serve did not return");
        result.unwrap().unwrap();
    }
}

pub fn ctx() -> Value {
    json!({ "cwd": "/work", "hasUI": true, "flags": {} })
}
