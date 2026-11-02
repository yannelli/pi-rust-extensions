//! The state of one agent run: when it started, what it does now, its tools and output tokens,
//! and the start and end times of each tool call.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::rows::Timing;

const CALLS_KEPT: usize = 512;

#[derive(Default)]
pub struct Run {
    pub started: Option<Instant>,
    pub label: String,
    pub tools: usize,
    pub output_tokens: u64,
    calls: HashMap<String, (Instant, Option<Duration>)>,
}

impl Run {
    pub fn start(&mut self, now: Instant) {
        self.started = Some(now);
        self.label = "Thinking".into();
        self.tools = 0;
        self.output_tokens = 0;
        if self.calls.len() > CALLS_KEPT {
            self.calls.clear();
        }
    }

    /// Time of the run so far, when it runs.
    pub fn elapsed(&self, now: Instant) -> Option<Duration> {
        self.started
            .map(|started| now.saturating_duration_since(started))
    }

    pub fn tool_started(&mut self, id: &str, tool: &str, args: &Value, now: Instant) {
        self.calls.insert(id.to_owned(), (now, None));
        self.tools += 1;
        self.label = label(tool, args);
    }

    pub fn tool_ended(&mut self, id: &str, now: Instant) {
        if let Some((started, took)) = self.calls.get_mut(id) {
            *took = Some(now.saturating_duration_since(*started));
        }
        self.label = "Working".into();
    }

    pub fn timing(&self, id: &str, now: Instant) -> Option<Timing> {
        let (started, took) = self.calls.get(id)?;
        Some(match took {
            Some(took) => Timing::Took(*took),
            None => Timing::Running(now.saturating_duration_since(*started)),
        })
    }

    /// `message.usage.output` of an assistant `message_end`.
    pub fn message_ended(&mut self, message: &Value) {
        if message.get("role").and_then(Value::as_str) == Some("assistant") {
            self.output_tokens += message
                .pointer("/usage/output")
                .and_then(Value::as_u64)
                .unwrap_or(0);
        }
    }
}

/// What the agent does while `tool` runs, for the working indicator.
pub fn label(tool: &str, args: &Value) -> String {
    let text = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or("");
    let file = |key: &str| text(key).rsplit('/').next().unwrap_or("").to_owned();
    let label = match tool {
        "bash" => format!(
            "Running {}",
            text("command")
                .split_whitespace()
                .take(3)
                .collect::<Vec<_>>()
                .join(" ")
        ),
        "read" => format!("Reading {}", file("path")),
        "edit" => format!("Editing {}", file("path")),
        "write" => format!("Writing {}", file("path")),
        "grep" | "find" => "Searching".into(),
        "ls" => "Listing files".into(),
        other => format!("Running {other}"),
    };
    short(label.trim(), 40)
}

/// `text` cut to `max` characters, the last one an ellipsis.
pub fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('\u{2026}');
    cut
}

/// `4s`, `1m 05s`, or `1h 02m`.
pub fn elapsed(time: Duration) -> String {
    let seconds = time.as_secs();
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60),
    }
}

/// `2.4k` style token counts.
pub fn tokens(count: u64) -> String {
    match count {
        0..1000 => count.to_string(),
        1000..1_000_000 => format!("{:.1}k", count as f64 / 1000.0),
        _ => format!("{:.1}M", count as f64 / 1_000_000.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn labels_and_formats() {
        assert_eq!(
            label("bash", &json!({ "command": "cargo test -p pi-tui --lib" })),
            "Running cargo test -p"
        );
        assert_eq!(
            label("edit", &json!({ "path": "/a/b/lib.rs" })),
            "Editing lib.rs"
        );
        assert_eq!(label("custom", &json!({})), "Running custom");
        assert_eq!(elapsed(Duration::from_secs(65)), "1m 05s");
        assert_eq!(elapsed(Duration::from_secs(3720)), "1h 02m");
        assert_eq!(tokens(2400), "2.4k");
        assert_eq!(short("abcdef", 4), "abc\u{2026}");
    }

    #[test]
    fn calls_time_from_start_to_end() {
        let mut run = Run::default();
        let start = Instant::now();
        run.start(start);
        run.tool_started("c1", "read", &json!({ "path": "x.rs" }), start);
        assert_eq!(run.label, "Reading x.rs");
        let later = start + Duration::from_secs(3);
        assert_eq!(
            run.timing("c1", later),
            Some(Timing::Running(Duration::from_secs(3)))
        );
        run.tool_ended("c1", later);
        assert_eq!(
            run.timing("c1", later + Duration::from_secs(5)),
            Some(Timing::Took(Duration::from_secs(3)))
        );
        run.message_ended(&json!({ "role": "assistant", "usage": { "output": 120 } }));
        run.message_ended(&json!({ "role": "user", "usage": { "output": 5 } }));
        assert_eq!((run.tools, run.output_tokens), (1, 120));
    }
}
