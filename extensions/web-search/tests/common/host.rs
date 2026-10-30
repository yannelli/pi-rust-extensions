use std::time::Duration;

use pi_ext_sdk::Extension;
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};
use tokio::task::JoinHandle;

/// A scripted pi host talking to the extension over an in-memory duplex.
pub struct TestHost {
    writer: Option<WriteHalf<DuplexStream>>,
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    server: JoinHandle<pi_ext_sdk::Result<()>>,
}

impl TestHost {
    pub fn start(ext: Extension) -> Self {
        let (host_side, ext_side) = tokio::io::duplex(1 << 20);
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

    /// The next message that is not a `tool.update` notification.
    pub async fn recv_skipping_updates(&mut self) -> Value {
        loop {
            let message = self.recv().await;
            if message["method"] != "tool.update" {
                return message;
            }
        }
    }

    pub async fn expect_request(&mut self, method: &str) -> (Value, Value) {
        let message = self.recv_skipping_updates().await;
        assert_eq!(message["method"], method, "unexpected message {message}");
        assert!(message.get("id").is_some(), "expected a request: {message}");
        (message["id"].clone(), message["params"].clone())
    }

    /// Answers the next `pi.call` for `target.method` with `result`.
    pub async fn answer_pi_call(&mut self, target: &str, method: &str, result: Value) -> Value {
        let (id, params) = self.expect_request("pi.call").await;
        assert_eq!(
            (params["target"].as_str(), params["method"].as_str()),
            (Some(target), Some(method)),
            "{params}"
        );
        self.answer(&id, result).await;
        params
    }

    pub async fn expect_response(&mut self, id: u64) -> Value {
        let message = self.recv_skipping_updates().await;
        assert_eq!(message["id"], id, "unexpected message {message}");
        message
    }

    pub async fn initialize(&mut self, agent_dir: &str, cwd: &str) -> Value {
        let params = json!({
            "protocolVersion": 1, "piVersion": "0.8.0", "cwd": cwd, "agentDir": agent_dir,
            "extensionPath": "native:test/web-search", "platform": "darwin", "mode": "tui",
            "hasUI": true, "flags": {}
        });
        self.request(0, "initialize", params).await;
        let response = self.expect_response(0).await;
        self.send(json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }))
            .await;
        response["result"].clone()
    }

    pub async fn finish(mut self) {
        if let Some(mut writer) = self.writer.take() {
            writer.shutdown().await.unwrap();
        }
        let joined = tokio::time::timeout(Duration::from_secs(5), self.server).await;
        joined.expect("serve did not return").unwrap().unwrap();
    }
}

pub fn ctx(cwd: &str) -> Value {
    json!({ "cwd": cwd, "hasUI": true, "flags": {}, "isProjectTrusted": false, "sessionId": "s1" })
}
