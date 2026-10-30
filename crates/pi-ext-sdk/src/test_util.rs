//! An in-memory pi host for extension tests (feature `test-util`).

use std::time::Duration;

use crate::Extension;
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};
use tokio::task::JoinHandle;

/// A scripted pi host talking to an extension over an in-memory duplex.
pub struct TestHost {
    writer: Option<WriteHalf<DuplexStream>>,
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    server: JoinHandle<crate::Result<()>>,
    cwd: String,
    agent_dir: String,
    /// Notifications the extension sent while `drive` ran.
    pub notifications: Vec<Value>,
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
            cwd: "/work".into(),
            agent_dir: "/home/u/.pi/agent".into(),
            notifications: Vec::new(),
        }
    }

    /// `cwd` of `initialize`; `/work` by default.
    pub fn cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = cwd.into();
        self
    }

    /// `agentDir` of `initialize`; `/home/u/.pi/agent` by default.
    pub fn agent_dir(mut self, agent_dir: impl Into<String>) -> Self {
        self.agent_dir = agent_dir.into();
        self
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

    /// Sends request `id` and answers each request of the extension with `host(method, params)`
    /// until the response to `id` arrives. Notifications go to `self.notifications`.
    pub async fn drive<F>(&mut self, id: u64, method: &str, params: Value, mut host: F) -> Value
    where
        F: FnMut(&str, &Value) -> Value,
    {
        self.request(id, method, params).await;
        loop {
            let message = self.recv().await;
            match (
                message.get("method").and_then(Value::as_str),
                message.get("id"),
            ) {
                (Some(method), Some(request)) => {
                    let result = host(method, &message["params"]);
                    let request = request.clone();
                    self.answer(&request, result).await;
                }
                (Some(_), None) => self.notifications.push(message),
                (None, _) if message["id"] == id => return message,
                (None, _) => panic!("response to an unknown request: {message}"),
            }
        }
    }

    pub async fn expect_response(&mut self, id: u64) -> Value {
        let message = self.recv().await;
        assert_eq!(message["id"], id, "unexpected message {message}");
        message
    }

    pub async fn initialize(&mut self) -> Value {
        let params = json!({
            "protocolVersion": 1, "piVersion": "0.8.0", "cwd": self.cwd,
            "agentDir": self.agent_dir, "extensionPath": "native:test/test",
            "platform": "darwin", "mode": "tui", "hasUI": true, "flags": {}
        });
        self.request(0, "initialize", params).await;
        let response = self.expect_response(0).await;
        self.notify("initialized", json!({})).await;
        response["result"].clone()
    }

    /// Closes stdin of the extension and waits for `serve` to return.
    pub async fn finish(mut self) -> crate::Result<()> {
        if let Some(mut writer) = self.writer.take() {
            writer.shutdown().await.unwrap();
        }
        tokio::time::timeout(Duration::from_secs(5), self.server)
            .await
            .expect("serve did not return")
            .unwrap()
    }

    pub async fn join(self) -> crate::Result<()> {
        tokio::time::timeout(Duration::from_secs(5), self.server)
            .await
            .expect("serve did not return")
            .unwrap()
    }
}

/// A minimal ctx snapshot with a UI.
pub fn ctx() -> Value {
    json!({ "cwd": "/work", "hasUI": true, "flags": {} })
}
