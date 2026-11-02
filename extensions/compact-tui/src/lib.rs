//! A compact, animated TUI for pi-rust: one-line rows for the built-in tools with the output
//! behind a gutter, a working indicator that names what the agent does, a busy title, and a
//! summary of each run. The footer stays with pi or a status line extension.

pub mod diff;
pub mod paint;
pub mod rows;
pub mod run;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, Instant};

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::{ChatStyle, ToolCallRender, ToolResultRender};
use serde::Deserialize;

use paint::{Paint, working_frames};
use rows::Look;
use run::Run;

const WIDGET: &str = "compact-tui";
const FRAME_MS: u64 = 80;

/// `<agent dir>/ext/compact-tui.json`. Every part is on by default.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Config {
    /// Compact rows for the built-in tools.
    pub tools: bool,
    /// The working indicator with the current step, the time, and the tokens.
    pub working: bool,
    /// `● ` before the terminal title while the agent works.
    pub title: bool,
    /// `✻ Worked for 1m 05s · 3 tools` above the editor after a run.
    pub summary: bool,
    /// Spinners and the shimmer; `PI_REDUCED_MOTION=1` also turns them off.
    pub motion: bool,
    /// `›` before user messages and `●` before assistant messages.
    pub markers: bool,
    /// Finished thinking blocks fold to one line.
    pub thinking: bool,
    /// Rounded editor corners and side bars.
    pub editor: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tools: true,
            working: true,
            title: true,
            summary: true,
            motion: true,
            markers: true,
            thinking: true,
            editor: true,
        }
    }
}

impl Config {
    pub fn load(agent_dir: &Path, reduced_motion: bool) -> Self {
        let path = agent_dir.join("ext/compact-tui.json");
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let mut config: Config = serde_json::from_str(&text).unwrap_or_default();
        config.motion &= !reduced_motion;
        config
    }
}

/// The key text of the keybindings that hints name.
#[derive(Clone, Debug)]
struct Keys {
    expand: String,
    interrupt: String,
    thinking: String,
}

impl Default for Keys {
    fn default() -> Self {
        Self {
            expand: "ctrl+o".into(),
            interrupt: "esc".into(),
            thinking: "ctrl+t".into(),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct State {
    config: Config,
    home: Option<String>,
    paint: RwLock<Paint>,
    run: Mutex<Run>,
    keys: RwLock<Keys>,
    chat_style: AtomicBool,
    spinner: AtomicBool,
    working: AtomicBool,
    /// The label of the indicator frames pi has, and the last working message.
    shown: Mutex<(String, String)>,
}

impl State {
    fn paint(&self) -> Paint {
        self.paint
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn keys(&self) -> Keys {
        self.keys
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn look<T>(&self, id: Option<&str>, draw: impl FnOnce(&Look) -> T) -> T {
        let paint = self.paint();
        let timing = id.and_then(|id| lock(&self.run).timing(id, Instant::now()));
        let expand = self.keys().expand;
        let spinner = self.spinner.load(Ordering::Relaxed);
        draw(&Look {
            paint: &paint,
            home: self.home.as_deref(),
            spinner,
            expand_key: &expand,
            timing,
        })
    }

    /// Sends the indicator frames when the step changed and the message when it changed.
    async fn show_working(&self, host: &Host) -> Result<()> {
        if !self.working.load(Ordering::Relaxed) {
            return Ok(());
        }
        let (label, message) = {
            let run = lock(&self.run);
            let Some(time) = run.elapsed(Instant::now()) else {
                return Ok(());
            };
            let mut parts = vec![run::elapsed(time)];
            if run.output_tokens > 0 {
                parts.push(format!("↓ {} tokens", run::tokens(run.output_tokens)));
            }
            parts.push(format!("{} to interrupt", self.keys().interrupt));
            (
                format!("{}…", run.label),
                self.paint()
                    .fg("muted", &format!("({})", parts.join(" · "))),
            )
        };
        let (frames, changed) = {
            let mut shown = lock(&self.shown);
            let frames = (shown.0 != label)
                .then(|| working_frames(&self.paint(), &label, self.config.motion));
            let changed = shown.1 != message;
            *shown = (label, message.clone());
            (frames, changed)
        };
        if let Some(frames) = frames {
            host.set_working_indicator(Some(&frames), Some(FRAME_MS))
                .await?;
        }
        if changed {
            host.set_working_message(Some(&message)).await?;
        }
        Ok(())
    }

    /// The message markers, the folded thinking label, and the rounded editor.
    async fn apply_chat_style(&self, host: &Host) -> Result<()> {
        let config = &self.config;
        if !self.chat_style.load(Ordering::Relaxed)
            || !(config.markers || config.thinking || config.editor)
        {
            return Ok(());
        }
        let paint = self.paint();
        let marker = |mark: &str| {
            config
                .markers
                .then(|| format!("{} ", paint.fg("accent", mark)))
        };
        let style = ChatStyle {
            user_prefix: marker("›"),
            assistant_prefix: marker("●"),
            collapse_thinking: config.thinking,
            rounded_editor: config.editor,
        };
        host.set_chat_style(&style).await?;
        if config.thinking {
            let label = format!("∴ Thought · {} to show", self.keys().thinking);
            host.set_hidden_thinking_label(Some(&label)).await?;
        }
        Ok(())
    }

    async fn end_run(&self, host: &Host, aborted: bool) -> Result<()> {
        let (time, tools, tokens) = {
            let mut run = lock(&self.run);
            let time = run.elapsed(Instant::now());
            run.started = None;
            (time, run.tools, run.output_tokens)
        };
        if self.working.load(Ordering::Relaxed) {
            *lock(&self.shown) = Default::default();
            host.set_working_indicator::<&str>(None, None).await?;
            host.set_working_message(None).await?;
        }
        let Some(time) = time.filter(|time| self.config.summary && *time >= Duration::from_secs(1))
        else {
            return Ok(());
        };
        let verb = if aborted {
            "Stopped after"
        } else {
            "Worked for"
        };
        let mut parts = vec![format!("✻ {verb} {}", run::elapsed(time))];
        if tools > 0 {
            parts.push(format!("{tools} tool{}", if tools == 1 { "" } else { "s" }));
        }
        if tokens > 0 {
            parts.push(format!("↓ {} tokens", run::tokens(tokens)));
        }
        let line = self.paint().fg("muted", &parts.join(" · "));
        host.set_widget(WIDGET, [line], Placement::AboveEditor)
            .await
    }
}

/// Key text as people write it: `esc` for `escape`, the first of several keys.
fn key_text(keys: &str) -> String {
    let first = keys.split('/').next().unwrap_or(keys);
    first
        .split('+')
        .map(|part| if part == "escape" { "esc" } else { part })
        .collect::<Vec<_>>()
        .join("+")
}

/// `π - <session name> - <cwd name>`, the title pi shows, with `● ` while the agent works.
fn title(snapshot: &Snapshot, busy: bool) -> String {
    let cwd = Path::new(&snapshot.cwd)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    let mut parts = vec!["π".to_owned()];
    parts.extend(snapshot.session_name.clone());
    parts.extend(cwd);
    format!("{}{}", if busy { "● " } else { "" }, parts.join(" - "))
}

/// Whether the last assistant message of an `agent_end` stopped on an abort.
fn aborted(event: &Value) -> bool {
    let messages = event.get("messages").and_then(Value::as_array);
    let last = messages.and_then(|messages| {
        messages
            .iter()
            .rev()
            .find(|message| message["role"] == "assistant")
    });
    last.is_some_and(|message| message["stopReason"] == "aborted")
}

fn on<F, Fut>(ext: Extension, state: &Arc<State>, name: &str, handle: F) -> Extension
where
    F: Fn(Arc<State>, EventCtx) -> Fut + Send + Sync + Copy + 'static,
    Fut: std::future::Future<Output = Result<()>> + Send + 'static,
{
    let state = state.clone();
    ext.on(name, move |ev: EventCtx| handle(state.clone(), ev))
}

pub fn extension(config: Config, home: Option<String>) -> Extension {
    let state = Arc::new(State {
        config: config.clone(),
        home,
        paint: RwLock::default(),
        run: Mutex::default(),
        keys: RwLock::new(Keys::default()),
        chat_style: AtomicBool::new(false),
        spinner: AtomicBool::new(false),
        working: AtomicBool::new(false),
        shown: Mutex::default(),
    });
    let mut ext = Extension::new("compact-tui");
    if config.tools {
        for tool in rows::TOOLS {
            let (calls, results) = (state.clone(), state.clone());
            ext = ext
                .render_tool_call(tool, move |req: ToolCallRender, _| {
                    let state = calls.clone();
                    async move {
                        Ok(state.look(req.tool_call_id.as_deref(), |look| {
                            rows::call_row(&req, look)
                        }))
                    }
                })
                .render_tool_result(tool, move |req: ToolResultRender, _| {
                    let state = results.clone();
                    async move { Ok(state.look(None, |look| rows::result_row(&req, look))) }
                })
                .render_shell(tool, "self");
        }
    }
    ext = on(ext, &state, "agent_start", |state, ev| async move {
        lock(&state.run).start(Instant::now());
        let host = ev.host();
        if state.config.summary {
            host.clear_widget(WIDGET).await?;
        }
        if state.config.title {
            host.set_title(&title(ev.snapshot(), true)).await?;
        }
        state.show_working(host).await
    });
    ext = on(ext, &state, "turn_start", |state, ev| async move {
        lock(&state.run).label = "Thinking".into();
        state.show_working(ev.host()).await
    });
    ext = on(
        ext,
        &state,
        "tool_execution_start",
        |state, ev| async move {
            let event = ev.event();
            let id = event["toolCallId"].as_str().unwrap_or_default();
            let tool = event["toolName"].as_str().unwrap_or_default();
            lock(&state.run).tool_started(id, tool, &event["args"], Instant::now());
            state.show_working(ev.host()).await
        },
    );
    ext = on(ext, &state, "tool_execution_end", |state, ev| async move {
        let id = ev.event()["toolCallId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        lock(&state.run).tool_ended(&id, Instant::now());
        state.show_working(ev.host()).await
    });
    ext = on(ext, &state, "message_end", |state, ev| async move {
        lock(&state.run).message_ended(&ev.event()["message"]);
        Ok(())
    });
    ext = on(ext, &state, "agent_end", |state, ev| async move {
        let host = ev.host();
        state.end_run(host, aborted(ev.event())).await?;
        if state.config.title {
            host.set_title(&title(ev.snapshot(), false)).await?;
        }
        Ok(())
    });
    let (start, themed) = (state.clone(), state);
    ext.on_start(move |host: Host| {
        let state = start.clone();
        async move {
            let features = |name: &str| host.init().is_some_and(|init| init.has_feature(name));
            state.spinner.store(
                features("render.spinner") && state.config.motion,
                Ordering::Relaxed,
            );
            state.working.store(
                features("ui.working") && state.config.working,
                Ordering::Relaxed,
            );
            if features("ui.getTheme")
                && let Ok(theme) = host.theme().await
            {
                *state.paint.write().unwrap_or_else(PoisonError::into_inner) =
                    Paint::new(Some(theme));
            }
            let ids = ["app.tools.expand", "app.interrupt", "app.thinking.toggle"];
            if let Ok(keys) = host.keybindings(&ids).await {
                let key = |id: &str, default: &str| {
                    keys.get(id)
                        .filter(|key| !key.is_empty())
                        .map_or(default.into(), |key| key_text(key))
                };
                *state.keys.write().unwrap_or_else(PoisonError::into_inner) = Keys {
                    expand: key(ids[0], "ctrl+o"),
                    interrupt: key(ids[1], "esc"),
                    thinking: key(ids[2], "ctrl+t"),
                };
            }
            state
                .chat_style
                .store(features("ui.chatStyle"), Ordering::Relaxed);
            let _ = state.apply_chat_style(&host).await;
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let _ = state.show_working(&host).await;
                }
            });
            Ok(())
        }
    })
    .on_theme_change(move |theme: Theme, host: Host| {
        let state = themed.clone();
        async move {
            *state.paint.write().unwrap_or_else(PoisonError::into_inner) = Paint::new(Some(theme));
            lock(&state.shown).0.clear();
            state.apply_chat_style(&host).await?;
            state.show_working(&host).await
        }
    })
}

pub fn main() -> ExitCode {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".pi/agent")))
        .unwrap_or_default();
    let reduced =
        std::env::var("PI_REDUCED_MOTION").is_ok_and(|value| !value.is_empty() && value != "0");
    let config = Config::load(&agent_dir, reduced);
    extension(config, home.map(|home| home.display().to_string())).run()
}
