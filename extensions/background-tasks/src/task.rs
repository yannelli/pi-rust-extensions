use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const TASK_FILE: &str = "task.json";
pub const OUTPUT_FILE: &str = "output.log";
pub const EXIT_FILE: &str = "exit";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Running,
    Completed,
    Failed,
    Killed,
    TimedOut,
    Lost,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Running => "running",
            Status::Completed => "completed",
            Status::Failed => "failed",
            Status::Killed => "killed",
            Status::TimedOut => "timed out",
            Status::Lost => "lost",
        }
    }

    pub fn is_running(self) -> bool {
        self == Status::Running
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub cwd: String,
    #[serde(default)]
    pub is_agent: bool,
    #[serde(default)]
    pub pid: Option<u32>,
    /// Start time of `pid` from `ps`, to detect a reused pid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_start: Option<String>,
    pub dir: PathBuf,
    pub started_at: u64,
    #[serde(default)]
    pub ended_at: Option<u64>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    pub notify_on_completion: bool,
    pub trigger_on_completion: bool,
    pub survive_reload: bool,
    pub status: Status,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub notified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Task {
    pub fn output_path(&self) -> PathBuf {
        self.dir.join(OUTPUT_FILE)
    }

    pub fn exit_path(&self) -> PathBuf {
        self.dir.join(EXIT_FILE)
    }

    pub fn save(&self) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(self)?;
        write_atomic(&self.dir.join(TASK_FILE), &json)
    }

    /// Marks the task finished with `status`; returns false when it already finished.
    pub fn finish(&mut self, status: Status, exit_code: Option<i32>) -> bool {
        if !self.status.is_running() {
            return false;
        }
        self.status = status;
        self.exit_code = exit_code;
        self.ended_at = Some(now_ms());
        true
    }

    pub fn timed_out(&self, now: u64) -> bool {
        self.timeout_seconds
            .is_some_and(|secs| now.saturating_sub(self.started_at) >= secs.saturating_mul(1000))
    }

    /// The exit code the wrapper shell wrote, when it finished.
    pub fn read_exit_code(&self) -> Option<i32> {
        let text = fs::read_to_string(self.exit_path()).ok()?;
        text.trim().parse().ok()
    }

    pub fn elapsed_ms(&self, now: u64) -> u64 {
        self.ended_at.unwrap_or(now).saturating_sub(self.started_at)
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// A short id: 8 hex digits from the time, the pid and a counter.
pub fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seed = now_ms() ^ (u64::from(std::process::id()) << 20);
    let mut x = seed.wrapping_add(
        COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B9),
    );
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    format!("{:08x}", x as u32)
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    let mut options = fs::File::options();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(&tmp)?.write_all(bytes)?;
    fs::rename(tmp, path)
}

/// Creates `dir` and missing parents with mode 0700 on unix, and sets `dir` itself to 0700.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;
    #[cfg(unix)]
    fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    Ok(())
}

/// Loads every `<dir>/<id>/task.json`, oldest first; unreadable entries are skipped.
pub fn load_all(dir: &Path) -> Vec<Task> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut tasks: Vec<Task> = entries
        .flatten()
        .filter_map(|entry| fs::read(entry.path().join(TASK_FILE)).ok())
        .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
        .collect();
    tasks.sort_by_key(|task| task.started_at);
    tasks
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogSlice {
    pub text: String,
    pub bytes_read: u64,
    pub total: u64,
    pub truncated: bool,
}

/// Reads at most `max` bytes from the start (`tail == false`) or the end of `path`.
pub fn read_bounded(path: &Path, max: u64, tail: bool) -> io::Result<LogSlice> {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(LogSlice {
                text: String::new(),
                bytes_read: 0,
                total: 0,
                truncated: false,
            });
        }
        Err(err) => return Err(err),
    };
    let total = file.metadata()?.len();
    let take = total.min(max);
    if tail {
        file.seek(SeekFrom::Start(total - take))?;
    }
    let mut buf = Vec::with_capacity(take as usize);
    file.take(take).read_to_end(&mut buf)?;
    Ok(LogSlice {
        text: String::from_utf8_lossy(&buf).into_owned(),
        bytes_read: buf.len() as u64,
        total,
        truncated: take < total,
    })
}

pub fn format_duration(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m{:02}s", secs / 60, secs % 60),
        _ => format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60),
    }
}

pub fn format_size(bytes: u64) -> String {
    if bytes >= 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes}B")
    }
}

pub fn session_dir(agent_dir: &Path, session_id: Option<&str>) -> PathBuf {
    let key: String = session_id
        .unwrap_or("no-session")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    agent_dir.join("ext/background-tasks/sessions").join(key)
}

pub fn resolve_in<'a>(tasks: &'a [Task], query: &str) -> Result<&'a Task, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("task id required".into());
    }
    if let Some(task) = tasks.iter().find(|t| t.id == query) {
        return Ok(task);
    }
    let matches: Vec<&Task> = tasks.iter().filter(|t| t.id.starts_with(query)).collect();
    match matches.as_slice() {
        [task] => Ok(task),
        [] => Err(format!("no background task matches `{query}`")),
        _ => Err(format!(
            "`{query}` matches {} tasks; use more characters",
            matches.len()
        )),
    }
}

pub fn default_name(command: &str) -> String {
    let first: String = command
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ");
    if first.chars().count() > 40 {
        first.chars().take(40).collect()
    } else {
        first
    }
}
