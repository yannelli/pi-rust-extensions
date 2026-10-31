mod agent;
mod register;
mod session;
mod ui;
mod usage;

pub use agent::{ExecOutput, ExecRequest, LlmRequest, LlmResponse, ProviderAuth};
pub use session::{CustomMessage, DeliverAs};
pub use ui::{InputOptions, NotifyType, Placement};
pub use usage::{ProviderUsage, UsageCredits, UsageWindow};

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::bus::Subscriptions;
use crate::error::Result;
use crate::paths;
use crate::peer::{self, Answer, Peer};
use crate::registry::Registry;
use crate::snapshot::{InitParams, Snapshot};
use crate::surface::{SurfaceEvents, Surfaces};

pub(crate) struct Inner {
    pub(crate) peer: Peer,
    pub(crate) registry: RwLock<Registry>,
    pub(crate) init: RwLock<Option<InitParams>>,
    pub(crate) snapshot: RwLock<Option<Arc<Snapshot>>>,
    pub(crate) surfaces: Surfaces,
    pub(crate) subscriptions: Subscriptions,
    next_surface: AtomicU64,
}

/// Handle for extension-to-host requests. Cheap to clone; usable from any task.
#[derive(Clone)]
pub struct Host {
    pub(crate) inner: Arc<Inner>,
}

impl Host {
    pub(crate) fn new(peer: Peer, registry: Registry) -> Self {
        let inner = Inner {
            peer,
            registry: RwLock::new(registry),
            init: RwLock::new(None),
            snapshot: RwLock::new(None),
            surfaces: Surfaces::new(),
            subscriptions: Subscriptions::default(),
            next_surface: AtomicU64::new(1),
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Sends any request and waits for its result. Dropping the future cancels the request.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.inner.peer.request(method, params).await
    }

    pub async fn call_as<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T> {
        Ok(serde_json::from_value(self.call(method, params).await?)?)
    }

    /// Sends a request before returning; dropping the `PendingCall` cancels it.
    pub(crate) fn start_call(&self, method: &str, params: Value) -> Result<PendingCall> {
        let (id, rx) = self.inner.peer.start(method, params)?;
        Ok(PendingCall {
            host: self.clone(),
            id,
            rx: Some(rx),
        })
    }

    /// Sends a notification; pi runs the method and drops its result.
    pub fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.inner.peer.notify(method, params)
    }

    /// `pi.call`: any member of `pi`, `ctx`, `ctx.ui`, `ctx.sessionManager`, `ctx.modelRegistry`.
    pub async fn pi_call(&self, target: &str, method: &str, args: Vec<Value>) -> Result<Value> {
        let params = json!({ "target": target, "method": method, "args": args });
        self.call("pi.call", params).await
    }

    /// The `initialize` params; `None` before pi sent them.
    pub fn init(&self) -> Option<InitParams> {
        self.inner.init.read().expect("init lock").clone()
    }

    /// The ctx snapshot of the most recent request from pi.
    pub fn snapshot(&self) -> Option<Arc<Snapshot>> {
        self.inner.snapshot.read().expect("snapshot lock").clone()
    }

    pub fn has_ui(&self) -> bool {
        match self.snapshot() {
            Some(snapshot) => snapshot.has_ui,
            None => self.init().is_some_and(|init| init.has_ui),
        }
    }

    pub fn cwd(&self) -> PathBuf {
        let from_snapshot = self.snapshot().map(|s| s.cwd.clone());
        let cwd = from_snapshot.filter(|cwd| !cwd.is_empty()).or_else(|| {
            self.init()
                .map(|init| init.cwd)
                .filter(|cwd| !cwd.is_empty())
        });
        cwd.map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default()
    }

    /// `agentDir` from `initialize`, else [`paths::agent_dir`].
    pub fn agent_dir(&self) -> PathBuf {
        let dir = self.init().map(|init| init.agent_dir);
        dir.filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(paths::agent_dir)
    }

    /// `<cwd>/.pi`.
    pub fn project_dir(&self) -> PathBuf {
        paths::project_dir(self.cwd())
    }

    /// Resize and close events of widget and footer surfaces.
    pub fn surface_events(&self) -> SurfaceEvents {
        self.inner.surfaces.subscribe()
    }

    /// The last width pi sent for `surface` (`widget:<key>`, `footer`, or a panel id).
    pub fn surface_width(&self, surface: &str) -> Option<usize> {
        self.inner.surfaces.width(surface)
    }

    pub(crate) fn surfaces(&self) -> &Surfaces {
        &self.inner.surfaces
    }

    pub(crate) fn next_surface_id(&self) -> String {
        format!(
            "panel-{}",
            self.inner.next_surface.fetch_add(1, Ordering::Relaxed)
        )
    }

    pub(crate) fn set_init(&self, init: InitParams) {
        *self.inner.init.write().expect("init lock") = Some(init);
    }

    pub(crate) fn set_snapshot(&self, snapshot: Arc<Snapshot>) {
        *self.inner.snapshot.write().expect("snapshot lock") = Some(snapshot);
    }
}

pub(crate) struct PendingCall {
    host: Host,
    id: u64,
    rx: Option<Answer>,
}

impl PendingCall {
    pub(crate) async fn wait(mut self) -> Result<Value> {
        let rx = self.rx.take().ok_or_else(crate::Error::closed)?;
        let result = peer::answer(rx).await;
        self.id = 0;
        result
    }
}

impl Drop for PendingCall {
    fn drop(&mut self) {
        if self.id != 0 {
            self.host.inner.peer.abandon(self.id);
        }
    }
}

pub(crate) fn lines<I, S>(lines: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    lines.into_iter().map(Into::into).collect()
}

/// Drops top-level `null` fields so optional params arrive as absent.
pub(crate) fn without_nulls(mut value: Value) -> Value {
    if let Value::Object(map) = &mut value {
        map.retain(|_, v| !v.is_null());
    }
    value
}
