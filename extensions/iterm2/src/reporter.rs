//! Sends the agent state and the session to iTerm2 through `ui.writeTerminal`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::tokio::sync::OnceCell;

use crate::features::{self, Features};
use crate::sequences::{
    ProgramState, badge, current_dir, request_attention, state_color, state_name, tab_color,
    user_var,
};

const NOTIFY_MIN_SECONDS: f64 = 10.0;

/// What `main` reads from the process environment.
#[derive(Clone, Copy, Debug, Default)]
pub struct Terminal {
    pub iterm2: bool,
    pub tmux: bool,
}

#[derive(Clone, Copy, Debug)]
struct Loaded {
    features: Features,
    attention_after: Duration,
}

pub struct Reporter {
    terminal: Terminal,
    loaded: OnceCell<Loaded>,
    features: Mutex<Option<Features>>,
    sent: Mutex<HashMap<&'static str, String>>,
    run_started: Mutex<Option<Instant>>,
}

impl Reporter {
    pub fn new(terminal: Terminal) -> Self {
        Self {
            terminal,
            loaded: OnceCell::new(),
            features: Mutex::new(None),
            sent: Mutex::new(HashMap::new()),
            run_started: Mutex::new(None),
        }
    }

    pub fn terminal(&self) -> Terminal {
        self.terminal
    }

    /// The config value from the extension files or the old `terminal.iterm2` setting.
    pub async fn config_value(&self, host: &Host) -> Value {
        let global = features::read_config(&features::global_config_path(&host.agent_dir()));
        let project = features::read_config(&features::project_config_path(&host.cwd()));
        let terminal: Option<Value> = host.settings("terminal").await.unwrap_or(None);
        let legacy = terminal.as_ref().and_then(|t| t.get("iterm2")).cloned();
        features::resolve(global, project, legacy)
    }

    async fn load(&self, host: &Host) -> Loaded {
        *self
            .loaded
            .get_or_init(|| async {
                let terminal: Option<Value> = host.settings("terminal").await.unwrap_or(None);
                let seconds = terminal
                    .as_ref()
                    .and_then(|t| t.get("notifyMinSeconds"))
                    .and_then(Value::as_f64)
                    .filter(|s| s.is_finite() && *s >= 0.0)
                    .unwrap_or(NOTIFY_MIN_SECONDS);
                let value = self.config_value(host).await;
                Loaded {
                    features: Features::from_value(Some(&value)),
                    attention_after: Duration::try_from_secs_f64(seconds).unwrap_or(Duration::MAX),
                }
            })
            .await
    }

    /// The enabled features, or `None` outside iTerm2, without a TUI, or when all are off.
    async fn active(&self, host: &Host) -> Option<Features> {
        let tui = host
            .init()
            .is_some_and(|init| init.mode == "tui" && init.has_ui);
        if !self.terminal.iterm2 || !tui {
            return None;
        }
        let loaded = self.load(host).await;
        let features = self
            .features
            .lock()
            .expect("features lock")
            .unwrap_or(loaded.features);
        features.any().then_some(features)
    }

    /// Replaces the features after `/terminal-setup` changed the config.
    pub fn set_features(&self, features: Features) {
        *self.features.lock().expect("features lock") = Some(features);
    }

    async fn write(&self, host: &Host, key: &'static str, sequence: String) {
        let changed = {
            let mut sent = self.sent.lock().expect("sent lock");
            sent.insert(key, sequence.clone()).as_ref() != Some(&sequence)
        };
        if changed
            && !host
                .write_terminal(&sequence, self.terminal.tmux)
                .await
                .unwrap_or(false)
        {
            self.sent.lock().expect("sent lock").remove(key);
        }
    }

    /// The tab color and `user.pi_state` from the footer `runState`, then the session.
    pub async fn report(&self, host: &Host, snapshot: &Snapshot) {
        let Some(features) = self.active(host).await else {
            return;
        };
        let state = snapshot
            .footer
            .as_ref()
            .and_then(|footer| footer.get("runState"))
            .and_then(Value::as_str)
            .and_then(ProgramState::parse);
        if let Some(state) = state {
            if features.tab_color {
                self.write(host, "tab", tab_color(state_color(state))).await;
            }
            if features.user_vars {
                let var = user_var("pi_state", state_name(state));
                self.write(host, "pi_state", var).await;
            }
        }
        self.report_session(host, snapshot, features).await;
    }

    async fn report_session(&self, host: &Host, snapshot: &Snapshot, features: Features) {
        let cwd = Path::new(&snapshot.cwd);
        let project = cwd
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = snapshot
            .session_name
            .clone()
            .unwrap_or_else(|| project.clone());
        if features.badge {
            self.write(host, "badge", badge(&format!("π {name}"))).await;
        }
        if features.cwd && !snapshot.cwd.is_empty() {
            self.write(host, "cwd", current_dir(cwd)).await;
        }
        if features.user_vars {
            let model = snapshot
                .model
                .as_ref()
                .map(|m| m.id.clone())
                .unwrap_or_default();
            self.write(host, "pi_session", user_var("pi_session", &name))
                .await;
            self.write(host, "pi_project", user_var("pi_project", &project))
                .await;
            self.write(host, "pi_model", user_var("pi_model", &model))
                .await;
        }
    }

    pub fn run_started(&self) {
        *self.run_started.lock().expect("run lock") = Some(Instant::now());
    }

    /// When a run took at least `terminal.notifyMinSeconds`, asks iTerm2 for attention.
    pub async fn run_finished(&self, host: &Host) {
        let started = self.run_started.lock().expect("run lock").take();
        let Some(features) = self.active(host).await else {
            return;
        };
        let after = self.load(host).await.attention_after;
        if features.attention && started.is_some_and(|start| start.elapsed() >= after) {
            let _ = host
                .write_terminal(request_attention(), self.terminal.tmux)
                .await;
        }
    }

    /// On exit: the profile tab color and no badge.
    pub async fn reset(&self, host: &Host) {
        let Some(features) = self.active(host).await else {
            return;
        };
        if features.tab_color {
            let _ = host
                .write_terminal(&tab_color(None), self.terminal.tmux)
                .await;
        }
        if features.badge {
            let _ = host.write_terminal(&badge(""), self.terminal.tmux).await;
        }
        self.sent.lock().expect("sent lock").clear();
    }
}
