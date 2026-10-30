use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use pi_ext_sdk::{CancellationToken, Error, Result};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// One page WebSocket; calls run in order and events are skipped.
pub struct Page {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    cancel: CancellationToken,
}

pub fn page_url(port: u16, page_id: &str) -> String {
    format!("ws://127.0.0.1:{port}/devtools/page/{page_id}")
}

pub fn http_url(port: u16, path: &str) -> String {
    format!("http://127.0.0.1:{port}{path}")
}

pub fn error_text(error: &Value) -> String {
    let message = error["message"].as_str().unwrap_or("unknown error");
    match error["data"].as_str() {
        Some(data) => format!("{message} ({data})"),
        None => message.to_owned(),
    }
}

impl Page {
    pub async fn connect(port: u16, page_id: &str, cancel: &CancellationToken) -> Result<Self> {
        let url = page_url(port, page_id);
        let connect = tokio::time::timeout(
            Duration::from_secs(5),
            tokio_tungstenite::connect_async(url.as_str()),
        );
        let (socket, _) = connect
            .await
            .map_err(|_| Error::msg(format!("Timed out connecting to {url}")))?
            .map_err(|err| Error::msg(format!("Browser page is no longer available ({err}). Call find_roots and observe_ui again.")))?;
        Ok(Self {
            socket,
            next_id: 0,
            cancel: cancel.clone(),
        })
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({ "id": id, "method": method, "params": params });
        self.socket
            .send(Message::Text(request.to_string().into()))
            .await
            .map_err(|err| Error::msg(format!("CDP {method}: {err}")))?;
        let wait = async {
            while let Some(message) = self.socket.next().await {
                let message = message.map_err(|err| Error::msg(format!("CDP {method}: {err}")))?;
                let Message::Text(text) = message else {
                    continue;
                };
                let Ok(reply) = serde_json::from_str::<Value>(text.as_str()) else {
                    continue;
                };
                if reply["id"].as_u64() != Some(id) {
                    continue;
                }
                if let Some(error) = reply.get("error") {
                    return Err(Error::msg(format!("CDP {method}: {}", error_text(error))));
                }
                return Ok(reply["result"].clone());
            }
            Err(Error::msg(format!(
                "CDP {method}: the DevTools connection closed"
            )))
        };
        tokio::select! {
            reply = tokio::time::timeout(CALL_TIMEOUT, wait) => {
                reply.map_err(|_| Error::msg(format!("CDP {method} timed out after {}s", CALL_TIMEOUT.as_secs())))?
            }
            () = self.cancel.cancelled() => Err(Error::msg("Operation aborted.")),
        }
    }

    /// `Runtime.evaluate` by value; a thrown exception becomes an error.
    pub async fn evaluate(&mut self, expression: &str) -> Result<Value> {
        let params =
            json!({ "expression": expression, "returnByValue": true, "awaitPromise": true });
        let result = self.call("Runtime.evaluate", params).await?;
        if let Some(details) = result.get("exceptionDetails") {
            let text = details["exception"]["description"]
                .as_str()
                .or(details["text"].as_str())
                .unwrap_or("exception");
            return Err(Error::msg(format!("Evaluation threw: {text}")));
        }
        Ok(result["result"]["value"].clone())
    }
}

pub fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|err| Error::msg(err.to_string()))
}

pub async fn get_json(port: u16, path: &str) -> Result<Value> {
    let url = http_url(port, path);
    let response = client()?
        .get(&url)
        .send()
        .await
        .map_err(|err| Error::msg(format!("No DevTools endpoint at {url}: {err}")))?;
    response
        .json()
        .await
        .map_err(|err| Error::msg(format!("{url} did not return JSON: {err}")))
}
