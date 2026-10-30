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
    input_allowed: AtomicBool,
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

    /// True after the user allowed desktop input in this session.
    pub fn input_allowed(&self) -> bool {
        self.input_allowed.load(Ordering::SeqCst)
    }

    pub fn set_input_allowed(&self, allowed: bool) {
        self.input_allowed.store(allowed, Ordering::SeqCst);
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

/// `<state>/captures`, created or reset to mode 0700 so other users cannot read screenshots.
pub fn captures_dir(state: &std::path::Path) -> std::io::Result<PathBuf> {
    let dir = state.join("captures");
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
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

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn captures_dir_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let state = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(state.path().join("captures")).unwrap();
        let open = std::fs::Permissions::from_mode(0o777);
        std::fs::set_permissions(state.path().join("captures"), open).unwrap();
        let dir = super::captures_dir(state.path()).unwrap();
        assert_eq!(dir, state.path().join("captures"));
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
