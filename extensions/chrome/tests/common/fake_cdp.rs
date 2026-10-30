use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

/// A local DevTools endpoint: `/json/version` over HTTP and a CDP WebSocket on a second port.
#[derive(Clone)]
pub struct FakeCdp {
    pub http_port: u16,
    pub http_hits: Arc<AtomicUsize>,
    pub calls: Arc<Mutex<Vec<Value>>>,
    pub targets: Arc<Mutex<Vec<Value>>>,
}

impl FakeCdp {
    pub async fn start() -> Self {
        let ws_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ws_port = ws_listener.local_addr().unwrap().port();
        let http_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fake = Self {
            http_port: http_listener.local_addr().unwrap().port(),
            http_hits: Arc::default(),
            calls: Arc::default(),
            targets: Arc::new(Mutex::new(vec![json!({
                "targetId": "USER", "type": "page", "url": "https://user.example/", "title": "User tab", "attached": false
            })])),
        };
        let hits = fake.http_hits.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = http_listener.accept().await {
                hits.fetch_add(1, Ordering::SeqCst);
                let mut buffer = [0u8; 2048];
                let _ = stream.read(&mut buffer).await;
                let body = json!({
                    "Browser": "FakeChrome/1.0",
                    "webSocketDebuggerUrl": format!("ws://127.0.0.1:{ws_port}/devtools/browser/fake-1")
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        let server = fake.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = ws_listener.accept().await {
                let server = server.clone();
                tokio::spawn(async move { server.serve(stream).await });
            }
        });
        fake
    }

    async fn serve(self, stream: tokio::net::TcpStream) {
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        while let Some(Ok(message)) = socket.next().await {
            let Message::Text(text) = message else {
                continue;
            };
            let request: Value = serde_json::from_str(text.as_str()).unwrap();
            self.calls.lock().unwrap().push(request.clone());
            let reply = match self.answer(&request) {
                Ok(result) => json!({ "id": request["id"], "result": result }),
                Err(message) => {
                    json!({ "id": request["id"], "error": { "code": -32000, "message": message } })
                }
            };
            let _ = socket.send(Message::Text(reply.to_string().into())).await;
        }
    }

    fn answer(&self, request: &Value) -> Result<Value, String> {
        let params = &request["params"];
        let mut targets = self.targets.lock().unwrap();
        Ok(match request["method"].as_str().unwrap_or("") {
            "Browser.getVersion" => json!({ "product": "FakeChrome/1.0" }),
            "Target.getTargets" => json!({ "targetInfos": *targets }),
            "Target.createTarget" => {
                let id = format!("T{}", targets.len());
                targets.push(json!({ "targetId": id, "type": "page", "url": params["url"], "title": "", "attached": false }));
                json!({ "targetId": id })
            }
            "Target.attachToTarget" => {
                json!({ "sessionId": format!("S-{}", params["targetId"].as_str().unwrap()) })
            }
            "Target.closeTarget" => {
                targets.retain(|t| t["targetId"] != params["targetId"]);
                json!({ "success": true })
            }
            "Page.navigate" => json!({ "frameId": "F1", "loaderId": "L1" }),
            "Page.captureScreenshot" => {
                json!({ "data": base64::engine::general_purpose::STANDARD.encode(b"PNGDATA") })
            }
            "Runtime.evaluate" => return evaluate(params["expression"].as_str().unwrap_or("")),
            _ => json!({}),
        })
    }

    pub fn methods(&self) -> Vec<String> {
        let calls = self.calls.lock().unwrap();
        calls
            .iter()
            .map(|c| c["method"].as_str().unwrap().to_owned())
            .collect()
    }

    pub fn calls_of(&self, method: &str) -> Vec<Value> {
        let calls = self.calls.lock().unwrap();
        calls
            .iter()
            .filter(|c| c["method"] == method)
            .cloned()
            .collect()
    }
}

fn evaluate(expression: &str) -> Result<Value, String> {
    let value = if expression.contains("data-pi-uid") && expression.contains("__piUidNext") {
        json!({
            "url": "https://example.com/", "title": "Example", "total": 1,
            "scroll": { "y": 0, "height": 800, "viewport": 600 },
            "elements": [{ "uid": "e1", "role": "button", "name": "Sign in", "x": 10, "y": 20 }],
            "text": "Welcome"
        })
    } else if expression.contains("scrollIntoView") {
        json!([10, 20])
    } else if expression.contains("readyState") || expression.contains("el.focus()") {
        json!(true)
    } else if expression.contains("location.href") {
        json!(["https://example.com/", "Example"])
    } else if expression.contains("activeElement") {
        json!("typed text")
    } else if expression.starts_with("throw") {
        return Ok(json!({
            "result": { "type": "object" },
            "exceptionDetails": { "text": "Uncaught", "exception": { "description": "Error: boom" } }
        }));
    } else {
        json!({ "echo": expression })
    };
    Ok(json!({ "result": { "type": "object", "value": value } }))
}
