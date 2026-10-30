use std::time::Duration;

use pi_ext_sdk::prelude::*;
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};

use crate::extension;

pub struct Host {
    writer: WriteHalf<DuplexStream>,
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    pub requests: Vec<Value>,
}

impl Host {
    pub fn start() -> Self {
        let (client, server) = tokio::io::duplex(1 << 16);
        let (read, write) = tokio::io::split(server);
        tokio::spawn(extension().serve(read, write));
        let (read, writer) = tokio::io::split(client);
        Self {
            writer,
            lines: BufReader::new(read).lines(),
            requests: Vec::new(),
        }
    }

    pub async fn send(&mut self, message: Value) {
        self.writer
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
    }

    pub async fn recv(&mut self) -> Value {
        let line = tokio::time::timeout(Duration::from_secs(10), self.lines.next_line())
            .await
            .expect("timed out waiting for the extension")
            .unwrap()
            .expect("extension closed stdout");
        serde_json::from_str(&line).unwrap()
    }

    /// Answers extension requests with `{}` (recording them) until `method` arrives.
    pub async fn wait_request(&mut self, method: &str) -> Value {
        loop {
            let message = self.recv().await;
            if message.get("method").is_some() && message.get("id").is_some() {
                let id = message["id"].clone();
                let found = message["method"] == method;
                self.requests.push(message.clone());
                self.send(json!({"jsonrpc":"2.0","id":id,"result":{}}))
                    .await;
                if found {
                    return message["params"].clone();
                }
            }
        }
    }

    /// Sends a request and answers extension requests until its response arrives.
    pub async fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        loop {
            let message = self.recv().await;
            match (message.get("method"), message.get("id")) {
                (Some(_), Some(request_id)) => {
                    let request_id = request_id.clone();
                    self.requests.push(message.clone());
                    self.send(json!({"jsonrpc":"2.0","id":request_id,"result":{}}))
                        .await;
                }
                (None, Some(_)) if message["id"] == id => {
                    assert!(message.get("error").is_none(), "{message}");
                    return message["result"].clone();
                }
                _ => {}
            }
        }
    }

    pub async fn tool(&mut self, id: u64, ctx: &Value, name: &str, params: Value) -> String {
        let params = json!({"name":name,"params":params,"toolCallId":format!("c{id}"),"ctx":ctx});
        let result = self.call(id, "tool.execute", params).await;
        result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }
}
