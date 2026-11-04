//! The timers of the bound session and the loop that fires them.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use pi_ext_sdk::CancellationToken;
use pi_ext_sdk::prelude::*;
use tokio::sync::{Notify, broadcast};

use crate::clock::now_ms;
use crate::state::{self, Fire, Timers};

pub const WAKE_CHANNEL: &str = "agent:wake";
pub const STATUS_KEY: &str = "timers";

/// The longest the loop sleeps before it checks the clock again.
const MAX_WAIT: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Bound {
    session: Option<String>,
    path: Option<PathBuf>,
    timers: Timers,
    cancel: Option<CancellationToken>,
}

pub struct Runtime {
    bound: Mutex<Bound>,
    changed: Notify,
    /// The last status sent; held across the request so updates arrive in order.
    status: tokio::sync::Mutex<Option<Option<String>>>,
    /// Wakes running `sleep` calls from inside this process (user input, fired schedules).
    pub wake: broadcast::Sender<String>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            bound: Mutex::default(),
            changed: Notify::new(),
            status: tokio::sync::Mutex::default(),
            wake: broadcast::channel(16).0,
        }
    }
}

impl Runtime {
    fn lock(&self) -> MutexGuard<'_, Bound> {
        self.bound.lock().expect("timers lock")
    }

    /// Arms the timers of `session_id`, disarming another session's first. No-op when bound.
    pub fn bind(self: &Arc<Self>, host: &Host, session_id: Option<&str>) {
        let key = state::session_key(session_id);
        let cancel = CancellationToken::new();
        {
            let mut bound = self.lock();
            if bound.session.as_deref() == Some(key.as_str()) {
                return;
            }
            if let Some(old) = bound.cancel.take() {
                old.cancel();
            }
            let path = state::session_path(&host.agent_dir(), &key);
            let mut timers = state::load(&path);
            timers.rearm(now_ms());
            *bound = Bound {
                session: Some(key),
                path: Some(path),
                timers,
                cancel: Some(cancel.clone()),
            };
        }
        tokio::spawn(run(host.clone(), self.clone(), cancel));
    }

    /// Disarms the bound session; its file stays for the next bind.
    pub fn unbind(&self) {
        let mut bound = self.lock();
        if let Some(cancel) = bound.cancel.take() {
            cancel.cancel();
        }
        *bound = Bound::default();
    }

    pub fn read<T>(&self, read: impl FnOnce(&Timers) -> T) -> T {
        read(&self.lock().timers)
    }

    /// Changes the timers, saves the session file, and wakes the loop.
    pub fn update<T>(&self, change: impl FnOnce(&mut Timers) -> T) -> T {
        let result = {
            let mut bound = self.lock();
            let result = change(&mut bound.timers);
            if let Some(path) = &bound.path {
                if let Err(err) = state::save(path, &bound.timers) {
                    pi_ext_sdk::warn!("cannot save {}: {err}", path.display());
                }
            }
            result
        };
        self.changed.notify_one();
        result
    }

    /// Sets the footer status when its text changed.
    pub async fn refresh_status(&self, host: &Host) {
        let mut last = self.status.lock().await;
        let text = self.read(Timers::status);
        if last.as_ref() == Some(&text) {
            return;
        }
        *last = Some(text.clone());
        if let Err(err) = host.set_status(STATUS_KEY, text.as_deref()).await {
            pi_ext_sdk::debug!("timers status failed: {}", err.message());
        }
    }
}

async fn run(host: Host, runtime: Arc<Runtime>, cancel: CancellationToken) {
    runtime.refresh_status(&host).await;
    loop {
        let wait = runtime.read(Timers::next_due).map_or(MAX_WAIT, |due| {
            let ms = (due - now_ms()).max(0) as u64;
            Duration::from_millis(ms).min(MAX_WAIT)
        });
        tokio::select! {
            () = cancel.cancelled() => return,
            () = runtime.changed.notified() => {}
            () = tokio::time::sleep(wait) => tick(&host, &runtime, &cancel).await,
        }
        runtime.refresh_status(&host).await;
    }
}

async fn tick(host: &Host, runtime: &Runtime, cancel: &CancellationToken) {
    let now = now_ms();
    let idle = runtime.read(|t| t.heartbeat_due(now)) && host.is_idle().await.unwrap_or(false);
    if cancel.is_cancelled() {
        return;
    }
    for fire in runtime.update(|t| t.tick(now, idle)) {
        send(host, runtime, fire).await;
    }
}

async fn send(host: &Host, runtime: &Runtime, fire: Fire) {
    let source = fire.custom_type;
    let message = CustomMessage::new(source, fire.text)
        .display(true)
        .details(&fire.details)
        .deliver_as(DeliverAs::FollowUp)
        .trigger_turn(true);
    if let Err(err) = host.send_message(message).await {
        pi_ext_sdk::warn!("timers: cannot send {source}: {}", err.message());
        return;
    }
    // Heartbeats fire only while the agent is idle, so there is no sleep to wake.
    if source == state::SCHEDULE_MESSAGE {
        let reason = fire.details["name"]
            .as_str()
            .or(fire.details["id"].as_str())
            .unwrap_or_default()
            .to_owned();
        let _ = runtime.wake.send(format!("schedule {reason}"));
        let wake = json!({ "source": "schedule", "reason": reason });
        if let Err(err) = host.emit(WAKE_CHANNEL, wake).await {
            pi_ext_sdk::debug!("timers: agent:wake failed: {}", err.message());
        }
    }
}
