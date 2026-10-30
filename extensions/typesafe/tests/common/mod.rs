#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use pi_ext_sdk::Extension;
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};

/// A scripted pi host talking to the extension over an in-memory duplex.
pub struct TestHost {
    writer: WriteHalf<DuplexStream>,
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
}

impl TestHost {
    pub fn start(ext: Extension) -> Self {
        let (host_side, ext_side) = tokio::io::duplex(256 * 1024);
        let (ext_read, ext_write) = tokio::io::split(ext_side);
        tokio::spawn(ext.serve(ext_read, ext_write));
        let (read, writer) = tokio::io::split(host_side);
        Self {
            writer,
            lines: BufReader::new(read).lines(),
        }
    }

    pub async fn send(&mut self, message: Value) {
        self.writer
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

    pub async fn answer(&mut self, id: &Value, result: Value) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
            .await;
    }

    pub async fn recv(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(10), self.lines.next_line())
            .await
            .expect("timed out waiting for the extension")
            .unwrap()
            .expect("extension closed stdout");
        serde_json::from_str(&line).unwrap()
    }

    pub async fn expect_request(&mut self, method: &str) -> (Value, Value) {
        let message = self.recv().await;
        assert_eq!(message["method"], method, "unexpected message {message}");
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

    pub async fn initialize(&mut self, agent_dir: &Path, has_ui: bool) -> Value {
        let params = json!({
            "protocolVersion": 1, "piVersion": "0.8.0", "cwd": "/work",
            "agentDir": agent_dir, "extensionPath": "native:test/typesafe",
            "platform": "darwin", "mode": "tui", "hasUI": has_ui, "flags": {}
        });
        self.request(0, "initialize", params).await;
        let response = self.expect_response(0).await;
        self.notify("initialized", json!({})).await;
        response["result"].clone()
    }
}

pub fn ctx(has_ui: bool) -> Value {
    json!({ "cwd": "/work", "hasUI": has_ui, "flags": {} })
}

/// A directory under the system temp dir, removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("pi-ext-typesafe-it-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
