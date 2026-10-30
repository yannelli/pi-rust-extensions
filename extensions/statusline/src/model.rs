//! What the status line knows about the session, built from the ctx snapshot and footer data.

use std::path::{Path, PathBuf};

use pi_ext_sdk::Snapshot;
use serde_json::{Value, json};

/// Whether the agent works, compacts, waits for the user, or finished.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunState {
    #[default]
    Idle,
    Working,
    Compacting,
    Blocked,
    Done,
    Error,
}

impl RunState {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "idle" => Some(Self::Idle),
            "working" => Some(Self::Working),
            "compacting" => Some(Self::Compacting),
            "blocked" => Some(Self::Blocked),
            "done" => Some(Self::Done),
            "error" => Some(Self::Error),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RunState::Idle => "idle",
            RunState::Working => "working",
            RunState::Compacting => "compacting",
            RunState::Blocked => "blocked",
            RunState::Done => "done",
            RunState::Error => "error",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatusModel {
    pub provider: String,
    pub id: String,
    pub name: String,
    pub reasoning: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UsageTotals {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContextUsage {
    pub tokens: Option<u64>,
    pub context_window: u64,
    pub percent: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatusSnapshot {
    pub cwd: PathBuf,
    pub home: Option<PathBuf>,
    /// The name of the git repository folder.
    pub project: Option<String>,
    pub branch: Option<String>,
    pub session_id: String,
    pub session_name: Option<String>,
    pub session_file: Option<PathBuf>,
    pub totals: UsageTotals,
    pub cache_hit_rate: Option<f64>,
    pub context: Option<ContextUsage>,
    pub context_window: Option<u64>,
    pub auto_compact: bool,
    pub model: Option<StatusModel>,
    /// `off`, `minimal`, `low`, `medium`, `high`, or `xhigh`.
    pub thinking: String,
    pub routed: Option<(String, Option<String>)>,
    pub subscription: bool,
    pub run_state: RunState,
    pub version: String,
}

/// What the extension tracks beside the ctx snapshot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Extras {
    pub home: Option<PathBuf>,
    pub project: Option<String>,
    pub cache_hit_rate: Option<f64>,
    pub compacting: bool,
    pub auto_compact: bool,
    pub version: String,
}

fn u64_at(value: &Value, key: &str) -> Option<u64> {
    value
        .get(key)
        .and_then(Value::as_f64)
        .map(|n| n.max(0.0) as u64)
}

impl StatusSnapshot {
    pub fn from_ctx(ctx: &Snapshot, extras: &Extras) -> Self {
        let footer = ctx.footer.clone().unwrap_or(Value::Null);
        let stats = footer.get("sessionStats").cloned().unwrap_or(Value::Null);
        let f64_at = |key: &str| stats.get(key).and_then(Value::as_f64);
        let totals = UsageTotals {
            input: u64_at(&stats, "input").unwrap_or(0),
            output: u64_at(&stats, "output").unwrap_or(0),
            cache_read: u64_at(&stats, "cacheRead").unwrap_or(0),
            cache_write: u64_at(&stats, "cacheWrite").unwrap_or(0),
            cost: f64_at("cost").unwrap_or(0.0),
        };
        let model = ctx.model.as_ref().map(|model| StatusModel {
            provider: model.provider.clone(),
            id: model.id.clone(),
            name: model.name.clone().unwrap_or_else(|| model.id.clone()),
            reasoning: model.reasoning.unwrap_or(false),
        });
        let usage = ctx.context_usage.clone().unwrap_or(Value::Null);
        let context_window = ctx
            .model
            .as_ref()
            .and_then(|model| model.context_window)
            .or_else(|| u64_at(&stats, "contextWindow"))
            .or_else(|| u64_at(&usage, "contextWindow"))
            .filter(|window| *window > 0);
        let context = if usage.is_object() {
            Some(ContextUsage {
                tokens: u64_at(&usage, "tokens"),
                context_window: u64_at(&usage, "contextWindow")
                    .or(context_window)
                    .unwrap_or(0),
                percent: usage.get("percent").and_then(Value::as_f64),
            })
        } else if stats.is_object() {
            Some(ContextUsage {
                tokens: u64_at(&stats, "contextTokens"),
                context_window: context_window.unwrap_or(0),
                percent: f64_at("contextPercent"),
            })
        } else {
            None
        };
        let run_state = footer
            .get("runState")
            .and_then(Value::as_str)
            .and_then(RunState::parse)
            .unwrap_or_default();
        Self {
            cwd: PathBuf::from(&ctx.cwd),
            home: extras.home.clone(),
            project: extras.project.clone(),
            branch: footer
                .get("gitBranch")
                .and_then(Value::as_str)
                .map(str::to_string),
            session_id: ctx.session_id.clone().unwrap_or_default(),
            session_name: ctx.session_name.clone(),
            session_file: ctx.session_file.as_ref().map(PathBuf::from),
            totals,
            cache_hit_rate: extras.cache_hit_rate,
            context: context.filter(|c| c.tokens.is_some() || c.percent.is_some()),
            context_window,
            auto_compact: extras.auto_compact,
            model,
            thinking: ctx.thinking_level.clone().unwrap_or_else(|| "off".into()),
            routed: None,
            subscription: false,
            run_state: if extras.compacting {
                RunState::Compacting
            } else {
                run_state
            },
            version: extras.version.clone(),
        }
    }

    pub fn context_percent(&self) -> Option<f64> {
        self.context.as_ref().and_then(|usage| usage.percent)
    }

    pub fn context_headroom(&self) -> Option<u64> {
        Some(
            self.context_window?
                .saturating_sub(self.context.as_ref()?.tokens?),
        )
    }
}

/// The cache hit rate of an assistant message `usage`, in percent of the prompt tokens.
pub fn cache_hit_rate(usage: &Value) -> Option<f64> {
    let read = u64_at(usage, "cacheRead").unwrap_or(0);
    let prompt =
        u64_at(usage, "input").unwrap_or(0) + read + u64_at(usage, "cacheWrite").unwrap_or(0);
    (prompt > 0).then(|| read as f64 / prompt as f64 * 100.0)
}

pub fn format_tokens(count: u64) -> String {
    let count_f = count as f64;
    match count {
        0..1_000 => count.to_string(),
        1_000..10_000 => format!("{:.1}k", count_f / 1000.0),
        10_000..1_000_000 => format!("{}k", (count_f / 1000.0).round() as u64),
        1_000_000..10_000_000 => format!("{:.1}M", count_f / 1_000_000.0),
        _ => format!("{}M", (count_f / 1_000_000.0).round() as u64),
    }
}

/// The cwd with the home folder as `~`.
pub fn format_cwd(cwd: &Path, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return cwd.display().to_string();
    };
    match cwd.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        Err(_) => cwd.display().to_string(),
    }
}

/// The JSON a `command` status line reads on stdin. Field names follow Claude Code's status
/// line input where pi has the same data, so existing scripts keep working.
pub fn status_payload(snapshot: &StatusSnapshot) -> Value {
    let cwd = snapshot.cwd.display().to_string();
    let percent = snapshot.context_percent();
    let model = snapshot.model.as_ref().map(|model| {
        json!({ "id": model.id, "display_name": model.name, "provider": model.provider, "reasoning": model.reasoning })
    });
    let thinking = snapshot
        .model
        .as_ref()
        .is_some_and(|model| model.reasoning)
        .then(|| json!({ "enabled": snapshot.thinking != "off", "level": snapshot.thinking }));
    let routed = snapshot
        .routed
        .as_ref()
        .map(|(id, level)| json!({ "id": id, "thinking_level": level }));
    let totals = &snapshot.totals;
    json!({
        "hook_event_name": "Status",
        "session_id": snapshot.session_id,
        "session_name": snapshot.session_name,
        "transcript_path": snapshot.session_file.as_ref().map(|path| path.display().to_string()),
        "cwd": cwd,
        "workspace": { "current_dir": cwd, "project_dir": cwd },
        "git": { "branch": snapshot.branch, "project": snapshot.project },
        "run_state": snapshot.run_state.as_str(),
        "model": model,
        "routed_model": routed,
        "thinking": thinking,
        "version": snapshot.version,
        "cost": { "total_cost_usd": totals.cost, "subscription": snapshot.subscription },
        "tokens": {
            "input": totals.input,
            "output": totals.output,
            "cache_read": totals.cache_read,
            "cache_write": totals.cache_write,
            "cache_hit_percentage": snapshot.cache_hit_rate,
        },
        "context_window": {
            "total_input_tokens": snapshot.context.as_ref().and_then(|usage| usage.tokens),
            "total_output_tokens": totals.output,
            "context_window_size": snapshot.context_window,
            "used_percentage": percent,
            "remaining_percentage": percent.map(|percent| (100.0 - percent).max(0.0)),
            "remaining_tokens": snapshot.context_headroom(),
            "auto_compact": snapshot.auto_compact,
        },
    })
}
