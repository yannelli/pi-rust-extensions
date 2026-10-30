use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

type Reply = std::result::Result<Value, String>;
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>;

pub struct Cdp {
    outbox: mpsc::UnboundedSender<Message>,
    pending: Pending,
    next_id: AtomicU64,
    closed: Arc<AtomicBool>,
}

impl Cdp {
    pub async fn connect(url: &str) -> Result<Self> {
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(5),
            tokio_tungstenite::connect_async(url),
        )
        .await
        .map_err(|_| anyhow!("timed out connecting to {url}"))?
        .with_context(|| format!("cannot open the DevTools WebSocket {url}"))?;
        let (mut sink, mut stream) = socket.split();
        let (outbox, mut queue) = mpsc::unbounded_channel::<Message>();
        let pending: Pending = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        tokio::spawn(async move {
            while let Some(message) = queue.recv().await {
                if sink.send(message).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });
        let (reader_pending, reader_closed) = (pending.clone(), closed.clone());
        tokio::spawn(async move {
            while let Some(Ok(message)) = stream.next().await {
                match message {
                    Message::Text(text) => dispatch(&reader_pending, text.as_str()),
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            reader_closed.store(true, Ordering::SeqCst);
            let waiting: Vec<_> = reader_pending.lock().expect("pending").drain().collect();
            for (_, sender) in waiting {
                let _ = sender.send(Err("the DevTools connection closed".to_owned()));
            }
        });
        Ok(Self {
            outbox,
            pending,
            next_id: AtomicU64::new(1),
            closed,
        })
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst) || self.outbox.is_closed()
    }

    pub async fn call(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
        timeout: Duration,
    ) -> Result<Value> {
        if self.is_closed() {
            bail!("the DevTools connection is closed");
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let mut request = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().expect("pending").insert(id, sender);
        if self
            .outbox
            .send(Message::Text(request.to_string().into()))
            .is_err()
        {
            self.pending.lock().expect("pending").remove(&id);
            bail!("the DevTools connection is closed");
        }
        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(message))) => Err(anyhow!("CDP {method}: {message}")),
            Ok(Err(_)) => bail!("CDP {method}: the DevTools connection closed"),
            Err(_) => {
                self.pending.lock().expect("pending").remove(&id);
                bail!("CDP {method} timed out after {} ms", timeout.as_millis())
            }
        }
    }
}

impl Drop for Cdp {
    fn drop(&mut self) {
        let _ = self.outbox.send(Message::Close(None));
    }
}

fn dispatch(pending: &Pending, text: &str) {
    let Ok(message) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let Some(id) = message.get("id").and_then(Value::as_u64) else {
        return;
    };
    let Some(sender) = pending.lock().expect("pending").remove(&id) else {
        return;
    };
    let reply = match message.get("error") {
        Some(error) => Err(error_text(error)),
        None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
    };
    let _ = sender.send(reply);
}

fn error_text(error: &Value) -> String {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error");
    match error.get("data").and_then(Value::as_str) {
        Some(data) => format!("{message} ({data})"),
        None => message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_text_includes_data() {
        let error = json!({ "code": -32000, "message": "No node", "data": "id 4" });
        assert_eq!(error_text(&error), "No node (id 4)");
        assert_eq!(error_text(&json!({})), "unknown error");
    }

    #[tokio::test]
    async fn dispatch_routes_results_and_errors() {
        let pending: Pending = Arc::default();
        let (ok_tx, ok_rx) = oneshot::channel();
        let (err_tx, err_rx) = oneshot::channel();
        pending.lock().unwrap().insert(1, ok_tx);
        pending.lock().unwrap().insert(2, err_tx);
        dispatch(&pending, r#"{"method":"Page.loadEventFired","params":{}}"#);
        dispatch(&pending, r#"{"id":1,"result":{"a":1}}"#);
        dispatch(&pending, r#"{"id":2,"error":{"message":"bad"}}"#);
        assert_eq!(ok_rx.await.unwrap(), Ok(json!({ "a": 1 })));
        assert_eq!(err_rx.await.unwrap(), Err("bad".to_owned()));
    }
}
