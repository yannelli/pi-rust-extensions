//! Status line segments: what each one shows, its theme role, and its icon. Codex
//! `[tui] status_line` item names work as aliases, so a Codex list renders as is.

pub mod usage;

use std::sync::OnceLock;

use crate::model::{RunState, StatusSnapshot, format_cwd, format_tokens};
use crate::themes::Role;

pub use crate::items::{Item, ItemKind};

const BAR_CELLS: usize = 8;

macro_rules! segments {
    ($($variant:ident => $name:literal $(| $alias:literal)*),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Segment { $($variant),* }

        impl Segment {
            pub const ALL: &[Segment] = &[$(Segment::$variant),*];

            /// The name used in `statusLine.segments` and `statusLine.colors`.
            pub fn name(self) -> &'static str {
                match self { $(Segment::$variant => $name),* }
            }

            /// A segment name or a Codex item name.
            pub fn from_name(name: &str) -> Option<Self> {
                match name { $($name $(| $alias)* => Some(Segment::$variant),)* _ => None }
            }
        }
    };
}

segments! {
    Cwd => "cwd" | "current-dir",
    Dir => "dir",
    Project => "project" | "project-name",
    Branch => "branch" | "git-branch",
    Session => "session" | "thread-title" | "session-name",
    SessionId => "sessionId" | "session-id" | "thread-id",
    Model => "model",
    ModelWithReasoning => "modelWithReasoning" | "model-with-reasoning",
    Provider => "provider",
    Thinking => "thinking" | "reasoning",
    Tokens => "tokens",
    TokensIn => "tokensIn" | "total-input-tokens",
    TokensOut => "tokensOut" | "total-output-tokens",
    TokensUsed => "tokensUsed" | "used-tokens",
    Cache => "cache",
    CacheHit => "cacheHit",
    Cost => "cost",
    Context => "context",
    ContextUsed => "contextUsed" | "context-used",
    ContextRemaining => "contextRemaining" | "context-remaining",
    ContextWindow => "contextWindow" | "context-window-size",
    ContextBar => "contextBar",
    ContextTokens => "contextTokens" | "context-tokens",
    ContextHeadroom => "contextHeadroom" | "context-headroom" | "tokensRemaining",
    Usage => "usage",
    UsageReset => "usageReset",
    Version => "version" | "codex-version",
    RunState => "runState" | "run-state",
    Time => "time",
    User => "user",
    Host => "host",
    Logo => "logo",
    Fill => "fill",
}

impl Segment {
    pub fn role(self, snapshot: &StatusSnapshot) -> Role {
        let percent = snapshot.context_percent().unwrap_or(0.0);
        match self {
            Segment::Cwd | Segment::Dir | Segment::Project => Role::Path,
            Segment::Branch => Role::Git,
            Segment::Session | Segment::SessionId => Role::Session,
            Segment::Model | Segment::ModelWithReasoning | Segment::Provider | Segment::Logo => {
                Role::Model
            }
            Segment::Thinking => Role::Thinking,
            Segment::Tokens | Segment::TokensIn | Segment::TokensOut | Segment::TokensUsed => {
                Role::Tokens
            }
            Segment::Cache | Segment::CacheHit => Role::Cache,
            Segment::Cost => Role::Cost,
            Segment::Context
            | Segment::ContextUsed
            | Segment::ContextRemaining
            | Segment::ContextWindow
            | Segment::ContextTokens
            | Segment::ContextHeadroom
            | Segment::ContextBar => {
                if percent > 90.0 {
                    Role::ContextHigh
                } else if percent > 70.0 {
                    Role::ContextWarn
                } else {
                    Role::ContextOk
                }
            }
            Segment::Usage | Segment::UsageReset => {
                usage::segment_role(snapshot.model.as_ref().map(|model| model.provider.as_str()))
            }
            Segment::Version | Segment::User | Segment::Host | Segment::Fill => Role::Meta,
            Segment::RunState => Role::State,
            Segment::Time => Role::Time,
        }
    }
}

fn dir_name(path: &std::path::Path, home: Option<&std::path::Path>) -> String {
    if home.is_some_and(|home| home == path) {
        return "~".to_string();
    }
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn user_and_host() -> &'static (Option<String>, Option<String>) {
    static CELL: OnceLock<(Option<String>, Option<String>)> = OnceLock::new();
    CELL.get_or_init(|| {
        let env = |names: &[&str]| {
            names
                .iter()
                .find_map(|name| std::env::var(name).ok().filter(|v| !v.is_empty()))
        };
        let host = env(&["HOSTNAME", "COMPUTERNAME"]).or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|name| name.trim().to_string())
                .filter(|n| !n.is_empty())
        });
        (
            env(&["USER", "USERNAME"]),
            host.map(|host| host.split('.').next().unwrap_or(&host).to_string()),
        )
    })
}

/// What a segment shows, without its icon; `None` when it has nothing to show.
pub fn segment_text(segment: Segment, snapshot: &StatusSnapshot) -> Option<String> {
    let totals = &snapshot.totals;
    let percent = snapshot.context_percent();
    let join = |parts: Vec<Option<String>>| {
        let parts: Vec<String> = parts.into_iter().flatten().collect();
        (!parts.is_empty()).then(|| parts.join(" "))
    };
    let level = || {
        snapshot
            .model
            .as_ref()
            .filter(|model| model.reasoning)
            .map(|_| match snapshot.thinking.as_str() {
                "off" => "thinking off".to_string(),
                level => level.to_string(),
            })
    };
    match segment {
        Segment::Cwd => Some(format_cwd(&snapshot.cwd, snapshot.home.as_deref())),
        Segment::Dir => Some(dir_name(&snapshot.cwd, snapshot.home.as_deref())),
        Segment::Project => snapshot
            .project
            .clone()
            .or_else(|| Some(dir_name(&snapshot.cwd, None))),
        Segment::Branch => snapshot.branch.clone(),
        Segment::Session => snapshot.session_name.clone(),
        Segment::SessionId => {
            Some(snapshot.session_id.chars().take(8).collect()).filter(|id: &String| !id.is_empty())
        }
        Segment::Model => Some(match (&snapshot.model, &snapshot.routed) {
            (Some(model), Some((routed, _))) => format!("{} → {routed}", model.id),
            (Some(model), None) => model.id.clone(),
            (None, _) => "no-model".to_string(),
        }),
        Segment::ModelWithReasoning => {
            let model = segment_text(Segment::Model, snapshot)?;
            Some(match level() {
                Some(level) => format!("{model} {level}"),
                None => model,
            })
        }
        Segment::Provider => snapshot.model.as_ref().map(|model| model.provider.clone()),
        Segment::Thinking => level(),
        Segment::Tokens => join(vec![
            (totals.input > 0).then(|| format!("↑{}", format_tokens(totals.input))),
            (totals.output > 0).then(|| format!("↓{}", format_tokens(totals.output))),
        ]),
        Segment::TokensIn => Some(format!("{} in", format_tokens(totals.input))),
        Segment::TokensOut => Some(format!("{} out", format_tokens(totals.output))),
        Segment::TokensUsed => {
            let used = totals.input + totals.output + totals.cache_read + totals.cache_write;
            Some(format!("{} used", format_tokens(used)))
        }
        Segment::Cache => join(vec![
            (totals.cache_read > 0).then(|| format!("R{}", format_tokens(totals.cache_read))),
            (totals.cache_write > 0).then(|| format!("W{}", format_tokens(totals.cache_write))),
            segment_text(Segment::CacheHit, snapshot),
        ]),
        Segment::CacheHit => snapshot
            .cache_hit_rate
            .filter(|_| totals.cache_read > 0 || totals.cache_write > 0)
            .map(|rate| format!("CH{rate:.1}%")),
        Segment::Cost => (totals.cost != 0.0 || snapshot.subscription).then(|| {
            format!(
                "${:.3}{}",
                totals.cost,
                if snapshot.subscription { " (sub)" } else { "" }
            )
        }),
        Segment::Context => {
            let window = format_tokens(snapshot.context_window?);
            let auto = if snapshot.auto_compact { " (auto)" } else { "" };
            Some(match percent {
                Some(percent) => format!("{percent:.1}%/{window}{auto}"),
                None => format!("?/{window}{auto}"),
            })
        }
        Segment::ContextUsed => percent.map(|percent| format!("{percent:.0}% used")),
        Segment::ContextRemaining => {
            percent.map(|percent| format!("{:.0}% left", (100.0 - percent).max(0.0)))
        }
        Segment::ContextWindow => snapshot
            .context_window
            .map(|window| format!("{} window", format_tokens(window))),
        Segment::ContextTokens => snapshot
            .context
            .as_ref()?
            .tokens
            .map(|tokens| format!("{} context", format_tokens(tokens))),
        Segment::ContextHeadroom => snapshot
            .context_headroom()
            .map(|tokens| format!("{} tokens left", format_tokens(tokens))),
        Segment::ContextBar => {
            snapshot.context_window?;
            let Some(percent) = percent else {
                return Some("context ?".into());
            };
            let percent = percent.clamp(0.0, 100.0);
            let filled = ((percent / 100.0) * BAR_CELLS as f64).round() as usize;
            Some(format!(
                "{}{} {percent:.0}%",
                "▰".repeat(filled),
                "▱".repeat(BAR_CELLS - filled)
            ))
        }
        Segment::Usage => usage::segment_text(&snapshot.model.as_ref()?.provider, false),
        Segment::UsageReset => usage::segment_text(&snapshot.model.as_ref()?.provider, true),
        Segment::Version => {
            (!snapshot.version.is_empty()).then(|| format!("v{}", snapshot.version))
        }
        Segment::RunState => Some(
            match snapshot.run_state {
                RunState::Idle => "○ idle",
                RunState::Working => "● working",
                RunState::Compacting => "◐ compacting",
                RunState::Blocked => "◆ blocked",
                RunState::Done => "✓ done",
                RunState::Error => "✗ error",
            }
            .to_string(),
        ),
        Segment::Time => Some(crate::clock::local_hhmm()),
        Segment::User => user_and_host().0.clone(),
        Segment::Host => user_and_host().1.clone(),
        Segment::Logo => Some("π".to_string()),
        Segment::Fill => None,
    }
}
