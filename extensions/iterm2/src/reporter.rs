//! Sends the agent state and the session to iTerm2 through `ui.writeTerminal`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::tokio::sync::OnceCell;

use crate::features::{self, Config, Features};
use crate::sequences::{
    ProgramState, badge, current_dir, request_attention, state_name, tab_color, user_var,
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
    config: Config,
    attention_after: Duration,
}

pub struct Reporter {
    terminal: Terminal,
    loaded: OnceCell<Loaded>,
    config: Mutex<Option<Config>>,
    state: Mutex<Option<ProgramState>>,
    sent: Mutex<HashMap<&'static str, String>>,
    run_started: Mutex<Option<Instant>>,
}

impl Reporter {
    pub fn new(terminal: Terminal) -> Self {
        Self {
            terminal,
            loaded: OnceCell::new(),
            config: Mutex::new(None),
            state: Mutex::new(None),
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
        features::resolve(global, project, self.legacy_value(host).await)
    }

    /// The old `terminal.iterm2` setting.
    pub async fn legacy_value(&self, host: &Host) -> Option<Value> {
        let terminal: Option<Value> = host.settings("terminal").await.unwrap_or(None);
        terminal.as_ref().and_then(|t| t.get("iterm2")).cloned()
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
                    config: Config::from_value(&value),
                    attention_after: Duration::try_from_secs_f64(seconds).unwrap_or(Duration::MAX),
                }
            })
            .await
    }

    /// The config, or `None` outside iTerm2, without a TUI, or when all features are off.
    async fn active(&self, host: &Host) -> Option<Config> {
        if !self.terminal.iterm2 || !is_tui(host) {
            return None;
        }
        let config = self.config(host).await;
        config.features.any().then_some(config)
    }

    async fn config(&self, host: &Host) -> Config {
        let loaded = self.load(host).await;
        self.config
            .lock()
            .expect("config lock")
            .unwrap_or(loaded.config)
    }

    /// Replaces the config after `/terminal-setup` or `/iterm2` changed it.
    pub fn set_config(&self, config: Config) {
        *self.config.lock().expect("config lock") = Some(config);
    }

    /// Shows `color` on the tab until `restore`; `None` shows the profile color.
    pub async fn preview(&self, host: &Host, color: Option<[u8; 3]>) {
        if self.terminal.iterm2 {
            self.write(host, "tab", tab_color(color)).await;
        }
    }

    /// The tab color of the last agent state, after a preview or a config change.
    pub async fn restore(&self, host: &Host) {
        if !self.terminal.iterm2 {
            return;
        }
        let state = *self.state.lock().expect("state lock");
        let color = match self.active(host).await {
            Some(config) if config.features.tab_color => {
                state.and_then(|state| config.palette.color(state))
            }
            _ => None,
        };
        self.write(host, "tab", tab_color(color)).await;
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

    /// After `/iterm2` changed the config: the tab color, the session features that turned on,
    /// and no badge when the badge pi set turned off.
    pub async fn apply(&self, host: &Host, snapshot: &Snapshot) {
        if !self.terminal.iterm2 {
            return;
        }
        let config = self.active(host).await;
        let shown = self
            .sent
            .lock()
            .expect("sent lock")
            .get("badge")
            .is_some_and(|sent| *sent != badge(""));
        if shown && !config.is_some_and(|c| c.features.badge) {
            self.write(host, "badge", badge("")).await;
        }
        if let Some(config) = config {
            self.report_session(host, snapshot, config.features).await;
        }
        self.restore(host).await;
    }

    /// The tab color and `user.pi_state` from the footer `runState`, then the session.
    pub async fn report(&self, host: &Host, snapshot: &Snapshot) {
        let Some(config) = self.active(host).await else {
            return;
        };
        let features = config.features;
        let state = snapshot
            .footer
            .as_ref()
            .and_then(|footer| footer.get("runState"))
            .and_then(Value::as_str)
            .and_then(ProgramState::parse);
        if let Some(state) = state {
            *self.state.lock().expect("state lock") = Some(state);
            if features.tab_color {
                let color = config.palette.color(state);
                self.write(host, "tab", tab_color(color)).await;
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
        let Some(config) = self.active(host).await else {
            return;
        };
        let after = self.load(host).await.attention_after;
        if config.features.attention && started.is_some_and(|start| start.elapsed() >= after) {
            let _ = host
                .write_terminal(request_attention(), self.terminal.tmux)
                .await;
        }
    }

    /// On exit: the profile tab color and no badge.
    pub async fn reset(&self, host: &Host) {
        let Some(Config { features, .. }) = self.active(host).await else {
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

/// The ctx snapshot has the current mode; older hosts send the load-time mode in `initialize`.
fn is_tui(host: &Host) -> bool {
    match host.snapshot() {
        Some(snapshot) if snapshot.mode.is_some() => {
            snapshot.mode.as_deref() == Some("tui") && snapshot.has_ui
        }
        _ => host
            .init()
            .is_some_and(|init| init.mode == "tui" && init.has_ui),
    }
}
