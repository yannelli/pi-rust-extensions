#![allow(dead_code)]

use std::time::Duration;

use pi_ext_sdk::Extension;
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};
use tokio::task::JoinHandle;

/// A scripted pi host talking to an extension over an in-memory duplex.
pub struct TestHost {
    writer: Option<WriteHalf<DuplexStream>>,
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    server: JoinHandle<pi_ext_sdk::Result<()>>,
}

impl TestHost {
    pub fn start(ext: Extension) -> Self {
        let (host_side, ext_side) = tokio::io::duplex(64 * 1024);
        let (ext_read, ext_write) = tokio::io::split(ext_side);
        let server = tokio::spawn(ext.serve(ext_read, ext_write));
        let (read, write) = tokio::io::split(host_side);
        Self {
            writer: Some(write),
            lines: BufReader::new(read).lines(),
            server,
        }
    }

    pub async fn send(&mut self, message: Value) {
        let writer = self.writer.as_mut().expect("writer open");
        let line = format!("{message}\n");
        writer.write_all(line.as_bytes()).await.unwrap();
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

    pub async fn recv(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(5), self.lines.next_line())
            .await
            .expect("timed out waiting for the extension")
            .unwrap()
            .expect("extension closed stdout");
        serde_json::from_str(&line).unwrap()
    }

    /// Receives the next message and checks it is a request for `method`.
    pub async fn expect_request(&mut self, method: &str) -> (Value, Value) {
        let message = self.recv().await;
        assert_eq!(message["method"], method, "unexpected message {message}");
        assert!(message.get("id").is_some(), "expected a request: {message}");
        (message["id"].clone(), message["params"].clone())
    }

    pub async fn expect_notification(&mut self, method: &str) -> Value {
        let message = self.recv().await;
        assert_eq!(message["method"], method, "unexpected message {message}");
        assert!(
            message.get("id").is_none(),
            "expected a notification: {message}"
        );
        message["params"].clone()
    }

    pub async fn expect_response(&mut self, id: u64) -> Value {
        let message = self.recv().await;
        assert_eq!(message["id"], id, "unexpected message {message}");
        message
    }

    pub async fn initialize(&mut self) -> Value {
        let params = json!({
            "protocolVersion": 1, "piVersion": "0.8.0", "cwd": "/work",
            "agentDir": "/home/u/.pi/agent", "extensionPath": "native:test/test",
            "platform": "darwin", "mode": "tui", "hasUI": true, "flags": {}
        });
        self.request(0, "initialize", params).await;
        let response = self.expect_response(0).await;
        self.notify("initialized", json!({})).await;
        response["result"].clone()
    }

    /// Closes stdin of the extension and waits for `serve` to return.
    pub async fn finish(mut self) -> pi_ext_sdk::Result<()> {
        if let Some(mut writer) = self.writer.take() {
            writer.shutdown().await.unwrap();
        }
        tokio::time::timeout(Duration::from_secs(5), self.server)
            .await
            .expect("serve did not return")
            .unwrap()
    }

    pub async fn join(self) -> pi_ext_sdk::Result<()> {
        tokio::time::timeout(Duration::from_secs(5), self.server)
            .await
            .expect("serve did not return")
            .unwrap()
    }
}

pub fn ctx() -> Value {
    json!({ "cwd": "/work", "hasUI": true, "flags": {} })
}

impl TestHost {
    /// Answers host requests with `null` (and `ui.custom` with `{ result: null }` once the
    /// extension closes the panel) until a message with `method` arrives; returns it.
    pub async fn pump_until(&mut self, method: &str, seen: &mut Vec<Value>) -> Value {
        loop {
            let message = self.recv().await;
            seen.push(message.clone());
            if message["method"] == method {
                return message;
            }
            self.auto_answer(&message, seen).await;
        }
    }

    /// Like `pump_until`, for the response to request `id`.
    pub async fn pump_response(&mut self, id: u64, seen: &mut Vec<Value>) -> Value {
        loop {
            let message = self.recv().await;
            if message["id"] == id && message.get("method").is_none() {
                return message;
            }
            seen.push(message.clone());
            self.auto_answer(&message, seen).await;
        }
    }

    async fn auto_answer(&mut self, message: &Value, seen: &[Value]) {
        let Some(method) = message["method"].as_str() else {
            return;
        };
        if message.get("id").is_none() {
            if method == "ui.close" {
                let custom = seen.iter().rev().find(|m| m["method"] == "ui.custom");
                if let Some(custom) = custom {
                    let id = custom["id"].clone();
                    self.answer(&id, json!({ "result": null })).await;
                }
            }
            return;
        }
        if method == "ui.custom" {
            return;
        }
        let result = if method == "ui.writeTerminal" {
            json!(true)
        } else {
            Value::Null
        };
        self.answer(&message["id"], result).await;
    }
}

pub fn emitted<'a>(seen: &'a [Value], channel: &str) -> Vec<&'a Value> {
    seen.iter()
        .filter(|m| m["method"] == "events.emit" && m["params"]["channel"] == channel)
        .map(|m| &m["params"]["data"])
        .collect()
}
