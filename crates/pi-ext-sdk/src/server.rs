use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::extension::Extension;
use crate::host::Host;
use crate::peer::{self, Peer};
use crate::protocol::{self, Incoming};
use crate::surface::SurfaceEvent;
use crate::{debug, dispatch, warn};

const SHUTDOWN_LIMIT: Duration = Duration::from_millis(1500);
const DRAIN_LIMIT: Duration = Duration::from_secs(2);

pub(crate) struct State {
    pub(crate) host: Host,
    running: Mutex<HashMap<String, CancellationToken>>,
    tasks: Mutex<JoinSet<()>>,
    shutdown_started: AtomicBool,
}

impl State {
    fn new(host: Host) -> Self {
        Self {
            host,
            running: Mutex::new(HashMap::new()),
            tasks: Mutex::new(JoinSet::new()),
            shutdown_started: AtomicBool::new(false),
        }
    }

    fn running(&self) -> std::sync::MutexGuard<'_, HashMap<String, CancellationToken>> {
        self.running.lock().expect("running lock")
    }

    /// Runs the `on_shutdown` hooks once, within the shutdown limit.
    pub(crate) async fn shutdown(&self) {
        if self.shutdown_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let hooks = self
            .host
            .inner
            .registry
            .read()
            .expect("registry lock")
            .on_shutdown
            .clone();
        let host = self.host.clone();
        let all = async move {
            for hook in hooks {
                if let Err(err) = hook(host.clone()).await {
                    warn!("shutdown hook failed: {err}");
                }
            }
        };
        if tokio::time::timeout(SHUTDOWN_LIMIT, all).await.is_err() {
            warn!("shutdown hooks did not finish in time");
        }
    }
}

pub(crate) async fn serve<R, W>(ext: Extension, reader: R, writer: W) -> Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (tx, rx) = mpsc::unbounded_channel();
    let writer_task = peer::spawn_writer(writer, rx);
    let state = Arc::new(State::new(Host::new(Peer::new(tx), ext.registry)));
    debug!("extension {} started", ext.name);
    let mut lines = BufReader::new(reader).lines();
    let outcome = loop {
        match lines.next_line().await {
            Ok(Some(line)) if line.trim().is_empty() => continue,
            Ok(Some(line)) => {
                if handle_line(&state, &line) == Flow::Exit {
                    break Ok(());
                }
            }
            Ok(None) => break Ok(()),
            Err(err) => break Err(err.into()),
        }
    };
    state.host.inner.peer.fail_all();
    drain(&state).await;
    state.shutdown().await;
    state.host.inner.surfaces.close_all();
    state.host.inner.subscriptions.close_all();
    state.host.inner.peer.close().await;
    let _ = writer_task.await;
    outcome
}

/// Lets requests read before `exit` or EOF answer, then cancels and drops the rest.
async fn drain(state: &State) {
    let mut tasks = std::mem::take(&mut *state.tasks.lock().expect("tasks lock"));
    let all = async { while tasks.join_next().await.is_some() {} };
    if tokio::time::timeout(DRAIN_LIMIT, all).await.is_err() {
        warn!("dropping {} unfinished requests", tasks.len());
    }
    let tokens: Vec<CancellationToken> = state.running().drain().map(|(_, t)| t).collect();
    tokens.iter().for_each(CancellationToken::cancel);
}

#[derive(PartialEq, Eq)]
enum Flow {
    Continue,
    Exit,
}

fn handle_line(state: &Arc<State>, line: &str) -> Flow {
    let peer = &state.host.inner.peer;
    match protocol::parse(line) {
        Err((id, err)) => peer.respond(&id, Err(err)),
        Ok(Incoming::Response { id, result }) => peer.complete(&id, result),
        Ok(Incoming::Request { id, method, params }) => spawn_request(state, id, method, params),
        Ok(Incoming::Notification { method, params }) => {
            return notification(state, &method, params);
        }
    }
    Flow::Continue
}

fn spawn_request(state: &Arc<State>, id: Value, method: String, params: Value) {
    let key = protocol::id_key(&id);
    let token = CancellationToken::new();
    state.running().insert(key.clone(), token.clone());
    let task_state = state.clone();
    let task = async move {
        let result = dispatch::handle(&task_state, &method, params, token).await;
        task_state.running().remove(&key);
        task_state.host.inner.peer.respond(&id, result);
    };
    let mut tasks = state.tasks.lock().expect("tasks lock");
    while tasks.try_join_next().is_some() {}
    tasks.spawn(task);
}

fn notification(state: &Arc<State>, method: &str, params: Value) -> Flow {
    let host = &state.host;
    let text = |key: &str| params.get(key).and_then(Value::as_str).unwrap_or_default();
    match method {
        "exit" => return Flow::Exit,
        "initialized" => start(host),
        "$/cancelRequest" => {
            let key = params.get("id").map(protocol::id_key).unwrap_or_default();
            if let Some(token) = state.running().get(&key) {
                token.cancel();
            }
        }
        "events.message" => {
            let data = params.get("data").cloned().unwrap_or(Value::Null);
            host.inner.subscriptions.deliver(text("channel"), &data);
        }
        "ui.resize" => {
            let width = params.get("width").and_then(Value::as_u64).unwrap_or(0);
            let event = host.inner.surfaces.resize(text("surface"), width as usize);
            surface_callbacks(host, event);
        }
        "ui.key" => host.inner.surfaces.key(text("surface"), text("data")),
        "ui.closed" => {
            let event = host.inner.surfaces.closed(text("surface"));
            surface_callbacks(host, event);
        }
        other => debug!("ignored notification {other}"),
    }
    Flow::Continue
}

fn start(host: &Host) {
    let hooks = host
        .inner
        .registry
        .read()
        .expect("registry lock")
        .on_start
        .clone();
    for hook in hooks {
        let host = host.clone();
        tokio::spawn(async move {
            if let Err(err) = hook(host).await {
                warn!("start hook failed: {err}");
            }
        });
    }
}

fn surface_callbacks(host: &Host, event: Option<SurfaceEvent>) {
    let Some(event) = event else { return };
    let callbacks = host
        .inner
        .registry
        .read()
        .expect("registry lock")
        .on_surface
        .clone();
    for callback in callbacks {
        let (host, event) = (host.clone(), event.clone());
        tokio::spawn(async move {
            if let Err(err) = callback(event, host).await {
                warn!("surface handler failed: {err}");
            }
        });
    }
}
