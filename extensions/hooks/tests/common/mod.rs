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

/// A ctx with a session, project trust, and the `hooks-trust-all` flag.
pub fn hooks_ctx(cwd: &std::path::Path, has_ui: bool, trust_all: bool) -> Value {
    json!({
        "cwd": cwd, "hasUI": has_ui, "sessionId": "s1", "isProjectTrusted": true,
        "flags": { "hooks-trust-all": trust_all }
    })
}

impl TestHost {
    /// Sends `event` and returns its response, answering `events.subscribe` on the way.
    pub async fn event(&mut self, id: u64, name: &str, event: Value, ctx: Value) -> Value {
        self.request(
            id,
            "event",
            json!({ "name": name, "event": event, "ctx": ctx }),
        )
        .await;
        self.next_skipping_subscribe().await
    }

    pub async fn next_skipping_subscribe(&mut self) -> Value {
        loop {
            let message = self.recv().await;
            if message["method"] == "events.subscribe" {
                self.answer(&message["id"].clone(), Value::Null).await;
                continue;
            }
            return message;
        }
    }
}
