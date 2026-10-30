use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use pi_ext_sdk::{Host, Snapshot};
use serde_json::Value;

use crate::browser::Managed;
use crate::config::{self, Config, Loaded};
use crate::state::Store;

#[derive(Default)]
pub struct Runtime {
    store: Mutex<Store>,
    loaded: Mutex<Loaded>,
    browser: Mutex<Option<Managed>>,
    pub live: tokio::sync::Mutex<()>,
    trust_noted: AtomicBool,
}

pub type Shared = Arc<Runtime>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

impl Runtime {
    pub fn store(&self) -> MutexGuard<'_, Store> {
        lock(&self.store)
    }

    pub fn browser(&self) -> MutexGuard<'_, Option<Managed>> {
        lock(&self.browser)
    }

    pub fn config(&self) -> Config {
        lock(&self.loaded).config
    }

    /// Reloads config; an untrusted project file is skipped and reported one time.
    pub async fn reload(&self, host: &Host, snapshot: Option<&Snapshot>) -> Loaded {
        let cwd = snapshot
            .map(Snapshot::cwd_path)
            .unwrap_or_else(|| host.cwd());
        let trusted = project_trusted(host, snapshot).await;
        let env = config::from_env(|key| std::env::var(key).ok());
        let loaded = config::load(&host.agent_dir(), &cwd, trusted, env);
        let untrusted = loaded
            .sources
            .iter()
            .any(|s| s.status == config::SourceStatus::Untrusted);
        if untrusted && !self.trust_noted.swap(true, Ordering::SeqCst) {
            let message = format!(
                "pi-computer-use ignored {} because the project is not trusted.",
                config::project_path(&cwd).display()
            );
            let _ = host.notify_warning(message).await;
        }
        *lock(&self.loaded) = loaded.clone();
        loaded
    }
}

pub fn state_dir(host: &Host) -> PathBuf {
    host.agent_dir().join("ext").join("computer-use")
}

/// Reads `isProjectTrusted` from the snapshot, then from `ctx`; unknown means untrusted.
pub async fn project_trusted(host: &Host, snapshot: Option<&Snapshot>) -> bool {
    let from_snapshot = snapshot
        .and_then(|s| s.extra.get("isProjectTrusted"))
        .and_then(Value::as_bool);
    if let Some(trusted) = from_snapshot {
        return trusted;
    }
    let answer = host.pi_call("ctx", "isProjectTrusted", Vec::new()).await;
    answer
        .ok()
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}
