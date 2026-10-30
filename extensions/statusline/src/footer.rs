//! Renders the active status line into the footer surface and runs `command` status lines.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use pi_ext_sdk::ExecRequest;
use pi_ext_sdk::prelude::*;

use crate::builtin::BuiltinStatusLine;
use crate::command::{DEBOUNCE, fit_output, run_command};
use crate::config::{StatusLineConfig, StatusLineSettings};
use crate::model::{Extras, StatusSnapshot, cache_hit_rate, status_payload};
use crate::sources;

pub const DEFAULT_WIDTH: usize = 80;

#[derive(Default)]
struct State {
    config: Option<StatusLineConfig>,
    ctx: Option<Snapshot>,
    extras: Extras,
    sent: Option<Vec<String>>,
    output: Option<Vec<String>>,
    command_key: Option<(String, usize)>,
    generation: u64,
    warned_untrusted: bool,
}

pub struct Footer {
    home: PathBuf,
    state: Mutex<State>,
}

impl Footer {
    pub fn new(home: PathBuf, version: String) -> Arc<Self> {
        let mut state = State::default();
        state.extras.home = Some(home.clone());
        state.extras.auto_compact = true;
        state.extras.version = version;
        Arc::new(Self {
            home,
            state: Mutex::new(state),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("footer lock")
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub(crate) fn trusted(host: &Host) -> bool {
        let snapshot = host.snapshot();
        let extra = snapshot
            .as_ref()
            .and_then(|s| s.extra.get("isProjectTrusted").and_then(Value::as_bool));
        extra.unwrap_or(false)
    }

    /// The global layer, including the legacy setting.
    pub fn global_value(host: &Host) -> Option<Value> {
        let agent_dir = host.agent_dir();
        let global = sources::read_object(&sources::global_config_path(&agent_dir));
        global
            .or_else(|| sources::legacy_global(&agent_dir))
            .map(Value::Object)
    }

    /// Reads the config files and the old setting, then renders.
    pub async fn reload(self: &Arc<Self>, host: &Host) {
        let agent_dir = host.agent_dir();
        let legacy_merged: Option<Value> = host.settings("statusLine").await.unwrap_or(None);
        let compaction: Option<Value> = host.settings("compaction").await.unwrap_or(None);
        let (global, project) = sources::layers(
            sources::read_object(&sources::global_config_path(&agent_dir)),
            sources::read_object(&sources::project_config_path(&host.cwd())),
            sources::legacy_global(&agent_dir),
            legacy_merged,
        );
        let effective = sources::effective(global, project, Self::trusted(host));
        let settings: StatusLineSettings =
            serde_json::from_value(effective.value).unwrap_or_default();
        let warn = {
            let mut state = self.lock();
            state.config = StatusLineConfig::from_settings(&settings, &self.home);
            state.extras.auto_compact = compaction
                .and_then(|c| c.get("enabled").and_then(Value::as_bool))
                .unwrap_or(true);
            state.command_key = None;
            state.output = None;
            let warn = effective.ignored_command && !state.warned_untrusted;
            state.warned_untrusted |= effective.ignored_command;
            warn
        };
        if warn {
            let text = "statusline: ignored the project status line command because the project is not trusted.";
            let _ = host.notify_warning(text).await;
        }
        self.render(host).await;
    }

    pub async fn find_project(self: &Arc<Self>, host: &Host) {
        let request = ExecRequest::new("git", &["rev-parse", "--show-toplevel"])
            .cwd(host.cwd().display().to_string())
            .timeout_ms(3000);
        let project = host
            .exec_with(request)
            .await
            .ok()
            .filter(|out| out.success())
            .and_then(|out| {
                let top = PathBuf::from(out.stdout.trim());
                top.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            });
        self.lock().extras.project = project;
    }

    /// Uses `snapshot` until an event brings a newer ctx.
    pub fn seed(&self, snapshot: &pi_ext_sdk::Snapshot) {
        let mut state = self.lock();
        if state.ctx.is_none() {
            state.ctx = Some(snapshot.clone());
        }
    }

    /// Records an event's ctx and what it says about compaction and the cache.
    pub fn observe(&self, ev: &EventCtx) {
        let mut state = self.lock();
        state.ctx = Some((*ev.snapshot()).clone());
        match ev.name() {
            "session_before_compact" => state.extras.compacting = true,
            "session_compact" => state.extras.compacting = false,
            "message_end" => {
                let message = ev.event().get("message").cloned().unwrap_or(Value::Null);
                if message.get("role").and_then(Value::as_str) == Some("assistant")
                    && let Some(usage) = message.get("usage")
                {
                    state.extras.cache_hit_rate = cache_hit_rate(usage);
                }
            }
            _ => {}
        }
    }

    pub fn snapshot(&self) -> Option<StatusSnapshot> {
        let state = self.lock();
        state
            .ctx
            .as_ref()
            .map(|ctx| StatusSnapshot::from_ctx(ctx, &state.extras))
    }

    /// Lines for the editor preview of an unsaved value.
    pub fn preview(&self, value: &Value, width: usize) -> Vec<String> {
        let settings: StatusLineSettings =
            serde_json::from_value(value.clone()).unwrap_or_default();
        let Some(snapshot) = self.snapshot() else {
            return Vec::new();
        };
        match StatusLineConfig::from_settings(&settings, &self.home) {
            Some(StatusLineConfig::Builtin(builtin)) => builtin.render(&snapshot, width),
            Some(StatusLineConfig::Command(_)) => vec![pi_ext_sdk::style::dim("(command output)")],
            None => vec![pi_ext_sdk::style::dim("(pi's default footer)")],
        }
    }

    pub fn set_version(&self, version: String) {
        self.lock().extras.version = version;
    }

    pub fn forget_sent(&self) {
        self.lock().sent = None;
    }

    pub fn has_refresh(&self) -> Option<std::time::Duration> {
        match &self.lock().config {
            Some(StatusLineConfig::Command(command)) => command.refresh,
            _ => None,
        }
    }

    /// Runs the command again although nothing changed.
    pub async fn refresh_command(self: &Arc<Self>, host: &Host) {
        self.lock().command_key = None;
        self.render(host).await;
    }

    fn render_boxed(self: Arc<Self>, host: Host) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move { self.render(&host).await })
    }

    pub async fn render(self: &Arc<Self>, host: &Host) {
        let tui = host
            .init()
            .is_some_and(|init| init.mode == "tui" && init.has_ui);
        if !tui {
            return;
        }
        if let Some(snapshot) = host.snapshot() {
            self.seed(&snapshot);
        }
        let width = host.surface_width("footer").unwrap_or(DEFAULT_WIDTH);
        let lines = {
            let mut state = self.lock();
            let Some(ctx) = state.ctx.clone() else { return };
            let snapshot = StatusSnapshot::from_ctx(&ctx, &state.extras);
            match state.config.clone() {
                None => None,
                Some(StatusLineConfig::Builtin(builtin)) => {
                    Some(render_builtin(&builtin, &snapshot, width))
                }
                Some(StatusLineConfig::Command(config)) => {
                    let command = &config;
                    let key = (status_payload(&snapshot).to_string(), width);
                    if state.command_key.as_ref() != Some(&key) {
                        let first = state.command_key.is_none() && state.output.is_none();
                        state.command_key = Some(key.clone());
                        state.generation += 1;
                        let (footer, host, generation) =
                            (self.clone(), host.clone(), state.generation);
                        let cwd = snapshot.cwd.clone();
                        let command = command.clone();
                        tokio::spawn(async move {
                            if !first {
                                tokio::time::sleep(DEBOUNCE).await;
                            }
                            if footer.lock().generation != generation {
                                return;
                            }
                            let Some(output) = run_command(&command, &key.0, key.1, &cwd).await
                            else {
                                return;
                            };
                            {
                                let mut state = footer.lock();
                                if state.generation != generation {
                                    return;
                                }
                                state.output = Some(output);
                            }
                            footer.render_boxed(host).await;
                        });
                    }
                    match &state.output {
                        Some(output) => Some(fit_output(output, config.padding, width)),
                        None => return,
                    }
                }
            }
        };
        self.send(host, lines).await;
    }

    async fn send(&self, host: &Host, lines: Option<Vec<String>>) {
        let unchanged = {
            let mut state = self.lock();
            let current = lines.clone().unwrap_or_default();
            let unchanged = match (&state.sent, &lines) {
                (None, None) => true,
                (Some(sent), Some(_)) => *sent == current,
                _ => false,
            };
            state.sent = lines.clone();
            unchanged
        };
        if unchanged {
            return;
        }
        let result = match lines {
            Some(lines) => host.set_footer(lines).await,
            None => host.clear_footer().await,
        };
        if let Err(error) = result {
            pi_ext_sdk::warn!("statusline: footer update failed: {error:?}");
            self.forget_sent();
        }
    }
}

fn render_builtin(
    builtin: &BuiltinStatusLine,
    snapshot: &StatusSnapshot,
    width: usize,
) -> Vec<String> {
    builtin.render(snapshot, width)
}
