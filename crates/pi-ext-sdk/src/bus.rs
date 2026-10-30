use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::error::Result;
use crate::host::Host;

type Senders = Vec<(u64, mpsc::UnboundedSender<Value>)>;

#[derive(Default)]
pub(crate) struct Subscriptions {
    next_id: AtomicU64,
    channels: Mutex<HashMap<String, Senders>>,
}

impl Subscriptions {
    /// Returns the subscription and whether it is the first one on `channel`.
    pub(crate) fn add(&self, host: Host, channel: &str) -> (Subscription, bool) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::unbounded_channel();
        let mut channels = self.channels.lock().expect("subscriptions lock");
        let senders = channels.entry(channel.to_owned()).or_default();
        senders.push((id, tx));
        let first = senders.len() == 1;
        let subscription = Subscription {
            host,
            channel: channel.to_owned(),
            id,
            rx,
        };
        (subscription, first)
    }

    /// Returns true when the last subscription on `channel` went away.
    fn remove(&self, channel: &str, id: u64) -> bool {
        let mut channels = self.channels.lock().expect("subscriptions lock");
        let Some(senders) = channels.get_mut(channel) else {
            return false;
        };
        senders.retain(|(existing, _)| *existing != id);
        let last = senders.is_empty();
        if last {
            channels.remove(channel);
        }
        last
    }

    pub(crate) fn deliver(&self, channel: &str, data: &Value) {
        let channels = self.channels.lock().expect("subscriptions lock");
        for (_, tx) in channels.get(channel).into_iter().flatten() {
            let _ = tx.send(data.clone());
        }
    }

    pub(crate) fn close_all(&self) {
        self.channels.lock().expect("subscriptions lock").clear();
    }
}

/// `pi.events` messages of one channel. Dropping the last one sends `events.unsubscribe`.
pub struct Subscription {
    host: Host,
    channel: String,
    id: u64,
    rx: mpsc::UnboundedReceiver<Value>,
}

impl Subscription {
    pub fn channel(&self) -> &str {
        &self.channel
    }

    /// The next message; `None` after the connection closes.
    pub async fn recv(&mut self) -> Option<Value> {
        self.rx.recv().await
    }

    pub async fn recv_as<T: DeserializeOwned>(&mut self) -> Option<Result<T>> {
        let data = self.recv().await?;
        Some(serde_json::from_value(data).map_err(Into::into))
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if self.host.inner.subscriptions.remove(&self.channel, self.id) {
            let params = json!({ "channel": self.channel });
            let _ = self.host.notify("events.unsubscribe", params);
        }
    }
}
