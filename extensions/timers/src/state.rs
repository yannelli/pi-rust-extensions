//! Heartbeats and schedules of one session, their firing rules, and the session file.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::clock::{format_duration, format_time, local};
use crate::cron::Cron;

pub const HEARTBEAT_MESSAGE: &str = "heartbeat";
pub const SCHEDULE_MESSAGE: &str = "schedule";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Heartbeat {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub reason: String,
    pub every_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pulses: Option<u32>,
    #[serde(default)]
    pub pulses: u32,
    pub next_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Trigger {
    At { at: i64 },
    Every { secs: u64 },
    Cron { expr: String },
}

impl Trigger {
    /// The fire time after one at `now`; `None` for a one-shot trigger.
    pub fn next_after(&self, now: i64) -> Option<i64> {
        match self {
            Self::At { .. } => None,
            Self::Every { secs } => Some(now + *secs as i64 * 1000),
            Self::Cron { expr } => Cron::parse(expr)
                .ok()?
                .next_after(local(now))
                .map(|at| at.timestamp_millis()),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::At { at } => format!("once at {}", format_time(*at)),
            Self::Every { secs } => format!("every {}", format_duration(*secs)),
            Self::Cron { expr } => format!("cron \"{expr}\""),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub prompt: String,
    pub trigger: Trigger,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_runs: Option<u32>,
    #[serde(default)]
    pub runs: u32,
    pub next_at: i64,
}

fn label(id: &str, name: Option<&str>) -> String {
    match name {
        Some(name) => format!("{id} \"{name}\""),
        None => id.to_owned(),
    }
}

impl Heartbeat {
    pub fn label(&self) -> String {
        label(&self.id, self.name.as_deref())
    }

    pub fn line(&self) -> String {
        let limit = self
            .max_pulses
            .map_or(String::new(), |max| format!(" of {max}"));
        format!(
            "heartbeat {} every {}, {} pulse(s){limit}, next {}: {}",
            self.label(),
            format_duration(self.every_secs),
            self.pulses,
            format_time(self.next_at),
            self.reason
        )
    }
}

impl Schedule {
    pub fn label(&self) -> String {
        label(&self.id, self.name.as_deref())
    }

    pub fn line(&self) -> String {
        let limit = self
            .max_runs
            .map_or(String::new(), |max| format!(" of {max}"));
        format!(
            "schedule {} {}, {} run(s){limit}, next {}: {}",
            self.label(),
            self.trigger.describe(),
            self.runs,
            format_time(self.next_at),
            self.prompt
        )
    }
}

/// A message to send when a timer fires.
#[derive(Debug, Clone, PartialEq)]
pub struct Fire {
    pub custom_type: &'static str,
    pub text: String,
    pub details: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Timers {
    pub heartbeats: Vec<Heartbeat>,
    pub schedules: Vec<Schedule>,
}

impl Timers {
    pub fn is_empty(&self) -> bool {
        self.heartbeats.is_empty() && self.schedules.is_empty()
    }

    pub fn next_due(&self) -> Option<i64> {
        let beats = self.heartbeats.iter().map(|h| h.next_at);
        beats.chain(self.schedules.iter().map(|s| s.next_at)).min()
    }

    pub fn heartbeat_due(&self, now: i64) -> bool {
        self.heartbeats.iter().any(|h| h.next_at <= now)
    }

    /// After a load: heartbeats that missed pulses wait one full interval. Overdue schedules
    /// stay due, so they fire once at the next tick.
    pub fn rearm(&mut self, now: i64) {
        for beat in &mut self.heartbeats {
            if beat.next_at <= now {
                beat.next_at = now + beat.every_secs as i64 * 1000;
            }
        }
    }

    /// Fires every due timer. Heartbeats pulse only when the agent is `idle`; a busy agent
    /// skips the pulse.
    pub fn tick(&mut self, now: i64, idle: bool) -> Vec<Fire> {
        let mut fires = Vec::new();
        self.heartbeats.retain_mut(|beat| {
            if beat.next_at > now {
                return true;
            }
            beat.next_at = now + beat.every_secs as i64 * 1000;
            if !idle {
                return true;
            }
            beat.pulses += 1;
            let last = beat.max_pulses.is_some_and(|max| beat.pulses >= max);
            fires.push(pulse(beat, last));
            !last
        });
        self.schedules.retain_mut(|schedule| {
            if schedule.next_at > now {
                return true;
            }
            schedule.runs += 1;
            let done = schedule.max_runs.is_some_and(|max| schedule.runs >= max);
            let next = if done {
                None
            } else {
                schedule.trigger.next_after(now)
            };
            fires.push(fired(schedule, next));
            match next {
                Some(next) => {
                    schedule.next_at = next;
                    true
                }
                None => false,
            }
        });
        fires
    }

    pub fn ids(&self) -> String {
        let beats = self.heartbeats.iter().map(|h| h.id.as_str());
        list_ids(beats.chain(self.schedules.iter().map(|s| s.id.as_str())))
    }

    pub fn ack(&mut self, id: &str) -> Result<Heartbeat, String> {
        match self.heartbeats.iter().position(|h| h.id == id) {
            Some(index) => Ok(self.heartbeats.remove(index)),
            None => Err(format!(
                "no heartbeat {id:?}; active heartbeats: {}",
                list_ids(self.heartbeats.iter().map(|h| h.id.as_str()))
            )),
        }
    }

    pub fn cancel(&mut self, id: &str) -> Result<Schedule, String> {
        match self.schedules.iter().position(|s| s.id == id) {
            Some(index) => Ok(self.schedules.remove(index)),
            None => Err(format!(
                "no schedule {id:?}; active schedules: {}",
                list_ids(self.schedules.iter().map(|s| s.id.as_str()))
            )),
        }
    }

    /// Removes a heartbeat or schedule; returns its line.
    pub fn remove(&mut self, id: &str) -> Result<String, String> {
        if let Ok(beat) = self.ack(id) {
            return Ok(format!("heartbeat {}", beat.label()));
        }
        if let Ok(schedule) = self.cancel(id) {
            return Ok(format!("schedule {}", schedule.label()));
        }
        Err(format!("no timer {id:?}; active ids: {}", self.ids()))
    }

    pub fn status(&self) -> Option<String> {
        let count = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
        let mut parts = Vec::new();
        if !self.heartbeats.is_empty() {
            parts.push(count(self.heartbeats.len(), "heartbeat"));
        }
        if !self.schedules.is_empty() {
            parts.push(count(self.schedules.len(), "schedule"));
        }
        (!parts.is_empty()).then(|| format!("timers: {}", parts.join(", ")))
    }

    pub fn list_text(&self) -> String {
        if self.is_empty() {
            return "No heartbeats or schedules.".into();
        }
        let beats = self.heartbeats.iter().map(Heartbeat::line);
        let lines: Vec<String> = beats
            .chain(self.schedules.iter().map(Schedule::line))
            .collect();
        lines.join("\n")
    }
}

fn list_ids<'a>(ids: impl Iterator<Item = &'a str>) -> String {
    let ids: Vec<&str> = ids.collect();
    if ids.is_empty() {
        "none".into()
    } else {
        ids.join(", ")
    }
}

fn pulse(beat: &Heartbeat, last: bool) -> Fire {
    let mut text = format!(
        "Heartbeat {} pulse {} (every {}).\nReason it was started: {}\n",
        beat.label(),
        beat.pulses,
        format_duration(beat.every_secs),
        beat.reason
    );
    if last {
        text.push_str("This was the last pulse (max_pulses reached); the heartbeat has stopped. Continue the work if it is not finished.");
    } else {
        text.push_str(&format!(
            "If the work is not finished, continue it now. Once the work is finished or these pulses are no longer needed, call heartbeat with action \"ack\" and id \"{}\" to stop them.",
            beat.id
        ));
    }
    let details = json!({ "id": beat.id, "name": beat.name, "pulse": beat.pulses, "everySecs": beat.every_secs, "last": last });
    Fire {
        custom_type: HEARTBEAT_MESSAGE,
        text,
        details,
    }
}

fn fired(schedule: &Schedule, next: Option<i64>) -> Fire {
    let limit = schedule
        .max_runs
        .map_or(String::new(), |max| format!(" of {max}"));
    let mut text = format!(
        "Scheduled prompt {} fired (run {}{limit}, {}).\n\n{}",
        schedule.label(),
        schedule.runs,
        schedule.trigger.describe(),
        schedule.prompt
    );
    if let Some(next) = next {
        text.push_str(&format!(
            "\n\nThis schedule repeats; next run at {}. To stop it, call schedule with action \"cancel\" and id \"{}\".",
            format_time(next),
            schedule.id
        ));
    }
    let details =
        json!({ "id": schedule.id, "name": schedule.name, "run": schedule.runs, "nextAt": next });
    Fire {
        custom_type: SCHEDULE_MESSAGE,
        text,
        details,
    }
}

/// An 8-character hex id.
pub fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seed = crate::clock::now_ms() as u64 ^ (u64::from(std::process::id()) << 20);
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

/// `<agentDir>/ext/timers/sessions/<session-id>.json`.
pub fn session_path(agent_dir: &Path, session_id: &str) -> PathBuf {
    agent_dir
        .join("ext/timers/sessions")
        .join(format!("{session_id}.json"))
}

/// A file-safe session key; `no-session` without an id.
pub fn session_key(session_id: Option<&str>) -> String {
    session_id
        .filter(|id| !id.is_empty())
        .unwrap_or("no-session")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn load(path: &Path) -> Timers {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Timers::default();
    };
    serde_json::from_str(&text).unwrap_or_else(|err| {
        pi_ext_sdk::warn!("timers file {} ignored: {err}", path.display());
        Timers::default()
    })
}

/// Writes the timers with a temp-file rename; removes the file when none are left.
pub fn save(path: &Path, timers: &Timers) -> std::io::Result<()> {
    if timers.is_empty() {
        return match std::fs::remove_file(path) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        };
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    let temp = path.with_file_name(format!(
        ".{}.{}.tmp",
        file_name.unwrap_or_default(),
        std::process::id()
    ));
    let mut file = std::fs::File::create(&temp)?;
    file.write_all(serde_json::to_string_pretty(timers)?.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&temp, path)
}
