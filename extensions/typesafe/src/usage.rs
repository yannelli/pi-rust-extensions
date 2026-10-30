use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use crate::caps::{Blocked, Caps};
use crate::credentials::{create_private_dir, write_private};

/// TypeSafe bills input tokens only; $42 per billion input tokens.
pub const DEFAULT_USD_PER_MTOK: f64 = 0.042;
pub const LOCK_WAIT: Duration = Duration::from_millis(200);
pub const LOCK_STALE: Duration = Duration::from_secs(5);
const KEEP_DAYS: usize = 31;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Totals {
    pub requests_started: u64,
    pub requests_succeeded: u64,
    pub requests_failed: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub costed_input_tokens: Option<u64>,
}

impl Totals {
    fn add(&self, delta: &Totals) -> Totals {
        let costed = self.cost_usd.is_some() || delta.cost_usd.is_some();
        let cost = self.cost_usd.unwrap_or(0.0) + delta.cost_usd.unwrap_or(0.0);
        Totals {
            requests_started: self.requests_started + delta.requests_started,
            requests_succeeded: self.requests_succeeded + delta.requests_succeeded,
            requests_failed: self.requests_failed + delta.requests_failed,
            input_tokens: self.input_tokens + delta.input_tokens,
            output_tokens: self.output_tokens + delta.output_tokens,
            cost_usd: costed.then(|| (cost * 1e12).round() / 1e12),
            costed_input_tokens: costed.then(|| {
                self.costed_input_tokens.unwrap_or(0) + delta.costed_input_tokens.unwrap_or(0)
            }),
        }
    }
}

/// Today's totals and estimated cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Today {
    pub day: String,
    pub totals: Totals,
    pub estimated_usd: f64,
}

pub type Clock = Arc<dyn Fn() -> String + Send + Sync>;

pub fn local_day() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

pub fn estimate_usd(input_tokens: u64, usd_per_mtok: f64) -> f64 {
    (input_tokens as f64 * usd_per_mtok / 1e6 * 1e6).round() / 1e6
}

/// `usage.json` daily counters with a `wx` lock file next to it, both owner-only.
pub struct Ledger {
    pub path: PathBuf,
    lock: PathBuf,
    usd_per_mtok: f64,
    clock: Clock,
    pending: Mutex<BTreeMap<String, Totals>>,
}

fn read_days(path: &Path) -> Option<BTreeMap<String, Totals>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(BTreeMap::new()),
        Err(_) => return None,
    };
    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let Some(days) = parsed.get("days").and_then(Value::as_object) else {
        return Some(BTreeMap::new());
    };
    let valid = |day: &str| {
        day.len() == 10
            && day.bytes().enumerate().all(|(i, b)| {
                matches!(i, 4 | 7) == (b == b'-') && (b == b'-' || b.is_ascii_digit())
            })
    };
    let entries = days.iter().filter(|(day, _)| valid(day));
    Some(
        entries
            .map(|(day, v)| {
                (
                    day.clone(),
                    serde_json::from_value(v.clone()).unwrap_or_default(),
                )
            })
            .collect(),
    )
}

struct LockGuard(PathBuf);

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Takes the lock within `LOCK_WAIT`; a lock older than `LOCK_STALE` is taken over.
fn take_lock(lock: &Path) -> Option<LockGuard> {
    create_private_dir(lock.parent()?).ok()?;
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        match options.open(lock) {
            Ok(_) => return Some(LockGuard(lock.to_owned())),
            Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => return None,
            Err(_) => {}
        }
        let modified = fs::metadata(lock).and_then(|m| m.modified());
        let age = modified.map(|m| SystemTime::now().duration_since(m).unwrap_or_default());
        if age.is_ok_and(|age| age > LOCK_STALE) {
            let _ = fs::remove_file(lock);
            continue;
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

impl Ledger {
    pub fn open(path: PathBuf, usd_per_mtok: f64, clock: Clock) -> Self {
        Self {
            lock: PathBuf::from(format!("{}.lock", path.display())),
            path,
            usd_per_mtok,
            clock,
            pending: Mutex::new(BTreeMap::new()),
        }
    }

    fn cost_of(&self, totals: &Totals) -> f64 {
        let uncosted = totals
            .input_tokens
            .saturating_sub(totals.costed_input_tokens.unwrap_or(0));
        let cost = totals.cost_usd.unwrap_or(0.0) + estimate_usd(uncosted, self.usd_per_mtok);
        (cost * 1e6).round() / 1e6
    }

    pub fn today(&self) -> Today {
        let day = (self.clock)();
        let stored = read_days(&self.path)
            .and_then(|mut d| d.remove(&day))
            .unwrap_or_default();
        let pending = self.pending.lock().expect("ledger lock");
        let totals = pending.get(&day).map_or(stored.clone(), |p| stored.add(p));
        let estimated_usd = self.cost_of(&totals);
        Today {
            day,
            totals,
            estimated_usd,
        }
    }

    /// Adds what this process holds to the file; a busy lock keeps it for the next update.
    pub fn flush(&self) {
        let mut pending = self.pending.lock().expect("ledger lock");
        if pending.is_empty() {
            return;
        }
        let Some(_release) = take_lock(&self.lock) else {
            return;
        };
        let Some(mut stored) = read_days(&self.path) else {
            return;
        };
        for (day, delta) in pending.iter() {
            let base = stored.remove(day).unwrap_or_default();
            stored.insert(day.clone(), base.add(delta));
        }
        let today = (self.clock)();
        let skip = stored.len().saturating_sub(KEEP_DAYS);
        let mut kept: BTreeMap<String, Totals> = stored.into_iter().skip(skip).collect();
        kept.entry(today).or_default();
        let text = json!({ "version": 1, "days": kept });
        let written = serde_json::to_string_pretty(&text)
            .map(|t| write_private(&self.path, &format!("{t}\n")));
        if matches!(written, Ok(Ok(()))) {
            pending.clear();
        }
    }

    fn add(&self, delta: Totals) {
        let day = (self.clock)();
        {
            let mut pending = self.pending.lock().expect("ledger lock");
            let base = pending.remove(&day).unwrap_or_default();
            pending.insert(day, base.add(&delta));
        }
        self.flush();
    }

    pub fn record_start(&self) {
        self.add(Totals {
            requests_started: 1,
            ..Totals::default()
        });
    }

    pub fn record_success(&self, input_tokens: u64, output_tokens: u64, usd: f64) {
        let costed = usd.is_finite() && usd >= 0.0;
        self.add(Totals {
            requests_succeeded: 1,
            input_tokens,
            output_tokens,
            cost_usd: costed.then_some(usd),
            costed_input_tokens: costed.then_some(input_tokens),
            ..Totals::default()
        });
    }

    pub fn record_failure(&self) {
        self.add(Totals {
            requests_failed: 1,
            ..Totals::default()
        });
    }

    pub fn blocked(&self, caps: &Caps) -> Option<Blocked> {
        let Today {
            day,
            totals,
            estimated_usd,
        } = self.today();
        let checks = [
            (
                "requestsPerDay",
                caps.max_requests_per_day.map(|v| v as f64),
                totals.requests_started as f64,
            ),
            (
                "inputTokensPerDay",
                caps.max_input_tokens_per_day.map(|v| v as f64),
                totals.input_tokens as f64,
            ),
            ("usdPerDay", caps.max_usd_per_day, estimated_usd),
        ];
        checks.into_iter().find_map(|(cap, limit, used)| {
            let limit = limit?;
            (used >= limit).then(|| Blocked {
                cap,
                limit,
                used,
                day: day.clone(),
            })
        })
    }
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
