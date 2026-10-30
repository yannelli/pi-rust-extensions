use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use pi_ext_sdk::{Host, Snapshot};
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::config::{self, LoadInput, Loaded};
use crate::model::HookDef;
use crate::paths::{Paths, state_dir};
use crate::trust::{LastRuns, Trust, TrustState, load_json, save_json};

pub const TRUST_ALL_FLAG: &str = "hooks-trust-all";
const ASYNC_LIMIT: usize = 8;

/// Per-session state shared by all handlers. Never held across an `.await`.
#[derive(Default)]
pub struct State {
    pub loaded: Option<Loaded>,
    pub trust: Option<TrustState>,
    pub last_runs: Option<LastRuns>,
    pub once_allowed: HashSet<String>,
    pub ran_once: HashSet<String>,
    pub held_context: Vec<String>,
    pub tool_context: HashMap<String, Vec<String>>,
    pub stop_hook_active: bool,
    pub last_assistant: Option<String>,
    pub last_error: Option<String>,
    pub warned_no_ui: bool,
    pub notified_untrusted: bool,
    pub reverting_model: Option<String>,
    pub turn: u64,
    pub spill: u64,
    pub flag_trust_all: bool,
}

pub struct Runtime {
    pub paths: Paths,
    state: Mutex<State>,
    pub async_slots: Arc<Semaphore>,
}

impl Runtime {
    pub fn new(paths: Paths) -> Arc<Self> {
        Arc::new(Self {
            paths,
            state: Mutex::new(State::default()),
            async_slots: Arc::new(Semaphore::new(ASYNC_LIMIT)),
        })
    }

    pub fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn agent_dir(&self, host: &Host) -> PathBuf {
        self.paths
            .agent_dir
            .clone()
            .unwrap_or_else(|| host.agent_dir())
    }

    fn state_file(&self, host: &Host, name: &str) -> PathBuf {
        state_dir(&self.agent_dir(host)).join(name)
    }

    /// Reloads all config files and notifies once when project files were skipped.
    pub async fn reload(&self, host: &Host, snap: &Snapshot) {
        let agent_dir = self.agent_dir(host);
        let cwd = PathBuf::from(&snap.cwd);
        let project_trusted = snap
            .extra
            .get("isProjectTrusted")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let input = LoadInput {
            paths: &self.paths,
            agent_dir: &agent_dir,
            cwd: &cwd,
            project_trusted,
        };
        let loaded = config::load(&input);
        let notice = {
            let mut state = self.lock();
            state.flag_trust_all = snap.flag_bool(TRUST_ALL_FLAG);
            if state.trust.is_none() {
                state.trust = Some(TrustState::load(&state_dir(&agent_dir).join("state.json")));
                state.last_runs = Some(
                    load_json(&state_dir(&agent_dir).join("last-runs.json")).unwrap_or_default(),
                );
            }
            let notice = (!loaded.untrusted.is_empty() && !state.notified_untrusted).then(|| {
                let files: Vec<String> = loaded
                    .untrusted
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect();
                format!(
                    "hooks: project is not trusted; skipped project hooks in {}",
                    files.join(", ")
                )
            });
            state.notified_untrusted |= notice.is_some();
            state.loaded = Some(loaded);
            notice
        };
        if let Some(notice) = notice {
            pi_ext_sdk::warn!("{notice}");
            let _ = host.notify_warning(notice).await;
        }
    }

    pub async fn ensure_loaded(&self, host: &Host, snap: &Snapshot) {
        let loaded = self.lock().loaded.is_some();
        if !loaded {
            self.reload(host, snap).await;
        }
    }

    pub fn hooks(&self) -> Vec<HookDef> {
        self.lock()
            .loaded
            .as_ref()
            .map(|l| l.hooks.clone())
            .unwrap_or_default()
    }

    pub fn trust_all(&self) -> bool {
        let state = self.lock();
        state.flag_trust_all || state.loaded.as_ref().is_some_and(|l| l.options.trust_all)
    }

    pub fn trust_level(&self, hash: &str) -> Trust {
        let trust_all = self.trust_all();
        let state = self.lock();
        let once = state.once_allowed.contains(hash);
        state
            .trust
            .as_ref()
            .map_or(Trust::Untrusted, |t| t.level(hash, trust_all, once))
    }

    /// Disabled in a scope's `disabled` list or in the older `state.json` list.
    pub fn is_disabled(&self, hash: &str) -> bool {
        let state = self.lock();
        let in_scope = state
            .loaded
            .as_ref()
            .is_some_and(|l| l.options.disabled.contains(hash));
        in_scope
            || state
                .trust
                .as_ref()
                .is_some_and(|t| t.disabled.contains(hash))
    }

    /// Changes the trust state and writes `state.json`.
    pub fn update_trust(&self, host: &Host, change: impl FnOnce(&mut TrustState)) {
        let path = self.state_file(host, "state.json");
        let mut state = self.lock();
        let trust = state.trust.get_or_insert_with(|| TrustState::load(&path));
        change(trust);
        trust.save(&path);
    }

    pub fn record_runs(&self, host: &Host, runs: Vec<(String, crate::trust::LastRun)>) {
        if runs.is_empty() {
            return;
        }
        let path = self.state_file(host, "last-runs.json");
        let mut state = self.lock();
        let all = state.last_runs.get_or_insert_with(LastRuns::new);
        all.extend(runs);
        save_json(&path, all);
    }

    pub fn last_run(&self, hash: &str) -> Option<crate::trust::LastRun> {
        self.lock()
            .last_runs
            .as_ref()
            .and_then(|runs| runs.get(hash).cloned())
    }

    /// Starts a new session: clears per-session state.
    pub fn reset_session(&self) {
        let mut state = self.lock();
        state.once_allowed.clear();
        state.ran_once.clear();
        state.held_context.clear();
        state.tool_context.clear();
        state.stop_hook_active = false;
        state.last_assistant = None;
        state.last_error = None;
    }

    /// Asks the user about an untrusted hook. `true` when it may run now.
    pub async fn resolve_trust(&self, host: &Host, def: &HookDef) -> bool {
        match self.trust_level(&def.hash) {
            Trust::All | Trust::Trusted | Trust::Once => return true,
            Trust::Denied => return false,
            Trust::Untrusted => {}
        }
        if !host.has_ui() {
            pi_ext_sdk::info!("hooks: skipping untrusted hook {def}");
            let mut state = self.lock();
            if !state.warned_no_ui {
                state.warned_no_ui = true;
                pi_ext_sdk::warn!(
                    "hooks: untrusted hooks are skipped without a UI; use --{TRUST_ALL_FLAG} or trustAll"
                );
            }
            return false;
        }
        let title = format!("Run untrusted hook?\n{}", def.trust_text());
        match host
            .select(&title, &["Allow once", "Always allow", "Deny"])
            .await
        {
            Ok(Some(choice)) if choice == "Allow once" => {
                self.lock().once_allowed.insert(def.hash.clone());
                true
            }
            Ok(Some(choice)) if choice == "Always allow" => {
                self.update_trust(host, |t| t.trust(&def.hash));
                true
            }
            Ok(Some(choice)) if choice == "Deny" => {
                self.update_trust(host, |t| t.deny(&def.hash));
                false
            }
            _ => false,
        }
    }
}
