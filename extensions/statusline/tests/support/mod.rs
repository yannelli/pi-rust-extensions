//! Shared fixtures for the ported status line tests.

#![allow(dead_code, unused_imports)]

pub use std::path::{Path, PathBuf};
pub use std::time::Duration;

pub use pi_ext_sdk::style::{strip, visible_width};
pub use pi_ext_statusline::builtin::BuiltinStatusLine;
pub use pi_ext_statusline::chips::Chip;
pub use pi_ext_statusline::command::{output_lines, run_command};
pub use pi_ext_statusline::config::*;
pub use pi_ext_statusline::external::{parse_codex_items, read_command};
pub use pi_ext_statusline::icons::Icons;
pub use pi_ext_statusline::model::*;
pub use pi_ext_statusline::presets::preset_names;
pub use pi_ext_statusline::segments::{Item, ItemKind, Segment, segment_text};
pub use pi_ext_statusline::themes::{Palette, Role, ink_for, theme_names};
pub use serde_json::json;

pub fn plain(lines: &[String]) -> Vec<String> {
    lines.iter().map(|line| strip(line)).collect()
}

pub const HOME: &str = "/nonexistent-home";

pub fn settings(value: serde_json::Value) -> StatusLineSettings {
    serde_json::from_value(value).expect("status line settings")
}

pub fn builtin(value: serde_json::Value) -> BuiltinStatusLine {
    match StatusLineConfig::from_settings(&settings(value), Path::new(HOME)) {
        Some(StatusLineConfig::Builtin(builtin)) => builtin,
        other => panic!("expected a builtin status line, got {other:?}"),
    }
}

pub fn first_line(value: serde_json::Value, width: usize) -> String {
    builtin(value)
        .render(&snapshot(), width)
        .into_iter()
        .next()
        .expect("a line")
}

pub fn plain_line(line: &str) -> String {
    plain(std::slice::from_ref(&line.to_string())).remove(0)
}

pub fn snapshot() -> StatusSnapshot {
    StatusSnapshot {
        cwd: PathBuf::from("/home/me/Workspace/node-cli"),
        home: Some(PathBuf::from("/home/me")),
        project: Some("node-cli".into()),
        branch: Some("format-double-quotes".into()),
        session_id: "0f3c9a7e-1111".into(),
        session_name: Some("quotes".into()),
        session_file: Some(PathBuf::from("/home/me/.pi/agent/sessions/s.jsonl")),
        totals: UsageTotals {
            input: 5100,
            output: 193,
            cache_read: 4500,
            cache_write: 0,
            cost: 0.013,
        },
        cache_hit_rate: Some(90.6),
        context: Some(ContextUsage {
            tokens: Some(13_500),
            context_window: 500_000,
            percent: Some(2.7),
        }),
        context_window: Some(500_000),
        auto_compact: true,
        model: Some(StatusModel {
            provider: "openai-codex".into(),
            id: "gpt-6.1-sol".into(),
            name: "GPT 6.1 Sol".into(),
            reasoning: true,
        }),
        thinking: "high".into(),
        routed: None,
        subscription: false,
        run_state: RunState::Working,
        version: "0.8.0".into(),
    }
}
