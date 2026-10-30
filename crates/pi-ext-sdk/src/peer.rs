use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde_json::{Value, json};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::error::{Error, Result};
use crate::protocol;

pub(crate) enum Out {
    Line(String),
    Close(oneshot::Sender<()>),
}

type Pending = Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>;
pub(crate) type Answer = oneshot::Receiver<Result<Value>>;

/// Writes protocol lines and tracks extension-to-host requests.
pub(crate) struct Peer {
    out: mpsc::UnboundedSender<Out>,
    next_id: AtomicU64,
    pending: Pending,
    reader_closed: AtomicBool,
}

impl Peer {
    pub(crate) fn new(out: mpsc::UnboundedSender<Out>) -> Self {
        Self {
            out,
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            reader_closed: AtomicBool::new(false),
        }
    }

    fn send_line(&self, line: String) -> Result<()> {
        self.out.send(Out::Line(line)).map_err(|_| Error::closed())
    }

    pub(crate) fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.send_line(protocol::notification(method, params))
    }

    pub(crate) fn respond(&self, id: &Value, result: Result<Value>) {
        let _ = self.send_line(protocol::response(id, result));
    }

    /// Registers and sends a request now; await the receiver for the answer.
    pub(crate) fn start(&self, method: &str, params: Value) -> Result<(u64, Answer)> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("pending lock").insert(id, tx);
        let sent = if self.reader_closed.load(Ordering::Acquire) {
            Err(Error::closed())
        } else {
            self.send_line(protocol::request(id, method, params))
        };
        if let Err(err) = sent {
            self.pending.lock().expect("pending lock").remove(&id);
            return Err(err);
        }
        Ok((id, rx))
    }

    /// Stops waiting for request `id` and sends `$/cancelRequest` when it was still pending.
    pub(crate) fn abandon(&self, id: u64) {
        let removed = self.pending.lock().expect("pending lock").remove(&id);
        if removed.is_some() {
            let _ = self.notify("$/cancelRequest", json!({ "id": id }));
        }
    }

    /// Dropping the returned future before the answer sends `$/cancelRequest`.
    pub(crate) async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let (id, rx) = self.start(method, params)?;
        let mut guard = CancelOnDrop {
            peer: self,
            id,
            armed: true,
        };
        let result = answer(rx).await;
        guard.armed = false;
        result
    }

    pub(crate) fn complete(&self, id: &Value, result: Result<Value>) {
        let Some(id) = id.as_u64() else { return };
        let waiter = self.pending.lock().expect("pending lock").remove(&id);
        if let Some(waiter) = waiter {
            let _ = waiter.send(result);
        }
    }

    /// Fails pending requests and every later one: pi can no longer answer.
    pub(crate) fn fail_all(&self) {
        self.reader_closed.store(true, Ordering::Release);
        self.pending.lock().expect("pending lock").clear();
    }

    pub(crate) async fn close(&self) {
        let (tx, rx) = oneshot::channel();
        if self.out.send(Out::Close(tx)).is_ok() {
            let _ = rx.await;
        }
    }
}

struct CancelOnDrop<'a> {
    peer: &'a Peer,
    id: u64,
    armed: bool,
}

impl Drop for CancelOnDrop<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.peer.abandon(self.id);
        }
    }
}

pub(crate) async fn answer(rx: Answer) -> Result<Value> {
    rx.await.unwrap_or_else(|_| Err(Error::closed()))
}

pub(crate) fn spawn_writer<W>(mut writer: W, mut rx: mpsc::UnboundedReceiver<Out>) -> JoinHandle<()>
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            match out {
                Out::Line(mut line) => {
                    line.push('\n');
                    let written = writer.write_all(line.as_bytes()).await;
                    if written.is_err() || writer.flush().await.is_err() {
                        break;
                    }
                }
                Out::Close(done) => {
                    let _ = writer.shutdown().await;
                    let _ = done.send(());
                    break;
                }
            }
        }
    })
}
