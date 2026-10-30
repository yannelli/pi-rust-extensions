#![allow(dead_code)]

pub mod fake_cdp;

use std::path::Path;
use std::time::Duration;

use pi_ext_sdk::Extension;
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};
use tokio::task::JoinHandle;

pub struct Pi {
    writer: Option<WriteHalf<DuplexStream>>,
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    server: JoinHandle<pi_ext_sdk::Result<()>>,
    pub active: Vec<String>,
    pub seen: Vec<Value>,
    pub confirm: bool,
    pub session_id: String,
    pub branch: Vec<Value>,
    cwd: String,
    next_id: u64,
}

impl Pi {
    pub fn start(ext: Extension, cwd: &Path) -> Self {
        let (host_side, ext_side) = tokio::io::duplex(1 << 20);
        let (ext_read, ext_write) = tokio::io::split(ext_side);
        let server = tokio::spawn(ext.serve(ext_read, ext_write));
        let (read, write) = tokio::io::split(host_side);
        Self {
            writer: Some(write),
            lines: BufReader::new(read).lines(),
            server,
            active: vec!["read".to_owned(), "bash".to_owned()],
            seen: Vec::new(),
            confirm: true,
            session_id: "s1".to_owned(),
            branch: Vec::new(),
            cwd: cwd.display().to_string(),
            next_id: 1,
        }
    }

    pub fn ctx(&self) -> Value {
        json!({ "cwd": self.cwd, "hasUI": true, "flags": {}, "sessionId": self.session_id, "isProjectTrusted": false })
    }

    async fn send(&mut self, message: Value) {
        let line = format!("{message}\n");
        let writer = self.writer.as_mut().expect("writer");
        writer.write_all(line.as_bytes()).await.unwrap();
    }

    pub async fn initialize(&mut self, agent_dir: &Path) -> Value {
        let params = json!({
            "protocolVersion": 1, "piVersion": "0.8.0", "cwd": self.cwd,
            "agentDir": agent_dir.display().to_string(), "extensionPath": "native:chrome",
            "platform": "darwin", "mode": "tui", "hasUI": true, "flags": {}
        });
        let result = self.request("initialize", params).await;
        self.send(json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }))
            .await;
        result["result"].clone()
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        loop {
            let message = self.recv().await;
            if message.get("method").is_none() && message["id"] == id {
                return message;
            }
            self.handle(message).await;
        }
    }

    async fn recv(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(10), self.lines.next_line())
            .await
            .expect("timed out waiting for the extension")
            .unwrap()
            .expect("extension closed stdout");
        serde_json::from_str(&line).unwrap()
    }

    async fn handle(&mut self, message: Value) {
        self.seen.push(message.clone());
        let Some(id) = message.get("id").cloned() else {
            return;
        };
        let params = &message["params"];
        let result = match message["method"].as_str().unwrap_or("") {
            "tools.getActive" => json!(self.active),
            "tools.setActive" => {
                self.active = serde_json::from_value(params["names"].clone()).unwrap();
                Value::Null
            }
            "ui.confirm" => json!(self.confirm),
            "session.getBranch" => json!(self.branch),
            "session.appendEntry" => {
                self.branch.push(json!({ "type": "custom", "customType": params["customType"], "data": params["data"] }));
                Value::Null
            }
            "pi.call" => json!(false),
            _ => Value::Null,
        };
        self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
            .await;
    }

    pub async fn event(&mut self, name: &str, event: Value) -> Value {
        let params = json!({ "name": name, "event": event, "ctx": self.ctx() });
        self.request("event", params).await
    }

    pub async fn command(&mut self, args: &str) -> Value {
        let params = json!({ "name": "chrome", "args": args, "ctx": self.ctx() });
        self.request("command.run", params).await
    }

    pub async fn tool(&mut self, name: &str, params: Value) -> Value {
        let id = format!("call-{}", self.next_id);
        let params = json!({ "toolCallId": id, "name": name, "params": params, "ctx": self.ctx() });
        self.request("tool.execute", params).await["result"].clone()
    }

    pub fn seen_methods(&self) -> Vec<String> {
        self.seen
            .iter()
            .filter_map(|m| m["method"].as_str().map(str::to_owned))
            .collect()
    }

    pub fn chrome_active(&self) -> usize {
        self.active
            .iter()
            .filter(|n| n.starts_with("chrome_"))
            .count()
    }

    pub async fn finish(mut self) {
        if let Some(mut writer) = self.writer.take() {
            writer.shutdown().await.unwrap();
        }
        let joined = tokio::time::timeout(Duration::from_secs(5), self.server).await;
        joined.expect("serve did not return").unwrap().unwrap();
    }
}

pub fn text(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_owned()
}
