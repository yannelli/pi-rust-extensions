#![allow(dead_code)]

pub mod http;
pub mod routing;

use std::path::PathBuf;
use std::time::Duration;

use base64::Engine;
use pi_ext_sdk::Extension;
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};
use tokio::task::JoinHandle;

/// A scripted pi host talking to an extension over an in-memory duplex (from the SDK tests).
pub struct TestHost {
    writer: Option<WriteHalf<DuplexStream>>,
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    server: JoinHandle<pi_ext_sdk::Result<()>>,
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
        let line = tokio::time::timeout(Duration::from_secs(10), self.lines.next_line())
            .await
            .expect("timed out waiting for the extension")
            .unwrap()
            .expect("extension closed stdout");
        serde_json::from_str(&line).unwrap()
    }

    pub async fn initialize(&mut self, agent_dir: &str) -> Value {
        let params = json!({
            "protocolVersion": 1, "piVersion": "0.8.0", "cwd": "/work",
            "agentDir": agent_dir, "extensionPath": "native:test/test",
            "platform": "darwin", "mode": "tui", "hasUI": true, "flags": {}
        });
        self.request(0, "initialize", params).await;
        let response = self.recv().await;
        assert_eq!(response["id"], 0, "{response}");
        self.notify("initialized", json!({})).await;
        response["result"].clone()
    }

    /// Answers every host request with `answer(method, params)` until the response to `id`.
    /// Returns the response and the requests seen, in order.
    pub async fn serve_until(
        &mut self,
        id: u64,
        mut answer: impl FnMut(&str, &Value) -> Value,
    ) -> (Value, Vec<(String, Value)>) {
        let mut seen = Vec::new();
        loop {
            let message = self.recv().await;
            if message.get("method").is_none() && message["id"] == id {
                return (message, seen);
            }
            let Some(method) = message["method"].as_str().map(str::to_owned) else {
                continue;
            };
            let params = message["params"].clone();
            if let Some(request_id) = message.get("id").cloned() {
                let result = answer(&method, &params);
                self.answer(&request_id, result).await;
            }
            seen.push((method, params));
        }
    }

    pub async fn finish(mut self) -> pi_ext_sdk::Result<()> {
        if let Some(mut writer) = self.writer.take() {
            writer.shutdown().await.unwrap();
        }
        tokio::time::timeout(Duration::from_secs(5), self.server)
            .await
            .expect("serve did not return")
            .unwrap()
    }
}

pub fn ctx(model: Option<(&str, &str)>) -> Value {
    let model = model.map(|(provider, id)| json!({ "provider": provider, "id": id }));
    json!({ "cwd": "/work", "hasUI": true, "flags": {}, "model": model })
}

/// A unique empty directory under the system temp dir.
pub fn temp_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("sr-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An unsigned JWT whose payload carries the ChatGPT account id claim.
pub fn codex_jwt(account: &str) -> String {
    let encode = |v: Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string().as_bytes())
    };
    let payload = json!({ "https://api.openai.com/auth": { "chatgpt_account_id": account } });
    format!(
        "{}.{}.sig",
        encode(json!({ "alg": "none" })),
        encode(payload)
    )
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
