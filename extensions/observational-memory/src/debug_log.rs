use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use serde_json::{Value, json};

pub const DEBUG_LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// NDJSON debug log under `<agentDir>/ext/observational-memory/`; a no-op when disabled.
#[derive(Debug, Clone, Default)]
pub struct DebugLog {
    pub path: Option<PathBuf>,
    pub cwd: String,
    pub session_id: Option<String>,
    pub session_file: Option<String>,
    pub run_id: Option<String>,
}

pub fn safe_session_id(session_id: Option<&str>) -> Option<String> {
    let mut collapsed = String::new();
    let mut in_run = false;
    for c in session_id?.trim().chars() {
        let allowed = c.is_ascii_alphanumeric() || "._-".contains(c);
        if allowed {
            collapsed.push(c);
        } else if !in_run {
            collapsed.push('_');
        }
        in_run = !allowed;
    }
    let trimmed: String = collapsed.trim_matches('_').chars().take(128).collect();
    trimmed
        .chars()
        .any(|c| c.is_ascii_alphanumeric())
        .then_some(trimmed)
}

pub fn debug_log_path(state_dir: &std::path::Path, session_id: Option<&str>) -> PathBuf {
    match safe_session_id(session_id) {
        Some(id) => state_dir.join("debug").join(format!("{id}.ndjson")),
        None => state_dir.join("debug.ndjson"),
    }
}

impl DebugLog {
    pub fn log(&self, event: &str, data: Value) {
        let Some(path) = &self.path else {
            return;
        };
        let payload = json!({
            "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "event": event,
            "cwd": self.cwd,
            "sessionId": self.session_id,
            "sessionFile": self.session_file,
            "runId": self.run_id,
            "data": data,
        });
        let _ = Self::append(path, &format!("{payload}\n"));
    }

    fn append(path: &PathBuf, line: &str) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        if fs::metadata(path).is_ok_and(|m| m.len() >= DEBUG_LOG_MAX_BYTES) {
            let backup = path.with_extension("ndjson.1");
            let _ = fs::remove_file(&backup);
            fs::rename(path, backup)?;
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?
            .write_all(line.as_bytes())
    }
}
