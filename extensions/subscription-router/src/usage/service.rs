//! Usage reads with a 5 minute cache, failure backoff, `retry-after`, and a 0600 `usage.json`.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::{Status, UsageSnapshot};
use crate::fsutil::{iso_seconds, now_ms, write_json};

const MANUAL_REFRESH_MS: i64 = 60_000;
const FRESH_REUSE_MS: i64 = 60_000;
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const FAILURE_BACKOFF_MS: [i64; 4] = [60_000, 120_000, 300_000, 900_000];
const RETRY_AFTER_CEILING_MS: i64 = 3_600_000;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Supplies the bearer token of a provider slot, and pi's own usage read when pi has one.
pub trait Tokens: Send + Sync {
    fn token<'a>(&'a self, provider: &'a str) -> BoxFuture<'a, Option<String>>;

    /// `usage.get` for `provider`; `refresh` skips pi's cache. The default reads here.
    fn core_usage<'a>(
        &'a self,
        _provider: &'a str,
        _refresh: bool,
    ) -> BoxFuture<'a, super::CoreRead> {
        Box::pin(async { super::CoreRead::Unsupported })
    }
}

#[derive(Clone, Debug)]
pub struct Urls {
    pub codex: String,
    pub anthropic: String,
}

impl Urls {
    pub fn from_env() -> Self {
        let var = |key: &str, default: &str| std::env::var(key).unwrap_or_else(|_| default.into());
        Self {
            codex: var(
                "PI_ROUTER_CODEX_USAGE_URL",
                "https://chatgpt.com/backend-api/wham/usage",
            ),
            anthropic: var(
                "PI_ROUTER_ANTHROPIC_USAGE_URL",
                "https://api.anthropic.com/api/oauth/usage",
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GetOptions {
    /// User-initiated; once per minute per provider.
    pub force: bool,
    /// Bypass the TTL but reuse a read from the last minute.
    pub fresh: bool,
    /// Reuse only reads that started at or after this time.
    pub not_before: Option<i64>,
}

#[derive(Default)]
struct Inner {
    cache: HashMap<String, UsageSnapshot>,
    manual: HashMap<String, i64>,
    backoff: HashMap<String, (usize, i64)>,
}

pub struct UsageService {
    pub(super) urls: Urls,
    pub(super) client: reqwest::Client,
    cache_file: Option<PathBuf>,
    inner: Mutex<Inner>,
    gates: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

pub(super) struct Outcome {
    pub(super) snapshot: UsageSnapshot,
    pub(super) failed: bool,
    pub(super) retry_after: Option<i64>,
}

impl UsageService {
    pub fn new(urls: Urls, cache_file: Option<PathBuf>) -> Self {
        let mut inner = Inner::default();
        let stored = cache_file
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok());
        let stored: Option<Value> = stored.and_then(|text| serde_json::from_str(&text).ok());
        if let Some(Value::Object(snapshots)) = stored.map(|v| v["snapshots"].clone()) {
            for (provider, value) in snapshots {
                if let Ok(snapshot) = serde_json::from_value(value) {
                    inner.cache.insert(provider, snapshot);
                }
            }
        }
        Self {
            urls,
            client: reqwest::Client::new(),
            cache_file,
            inner: Mutex::new(inner),
            gates: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Cached snapshot without a network read; expired data comes back `stale`.
    pub fn peek(&self, provider: &str) -> Option<UsageSnapshot> {
        let cached = self.lock().cache.get(provider).cloned()?;
        let now = now_ms();
        Some(if cached.expires_at > now {
            cached
        } else {
            cached.stale(now, Some("Cached usage data is stale.".into()))
        })
    }

    pub fn snapshots(&self) -> Vec<UsageSnapshot> {
        let mut providers: Vec<String> = self.lock().cache.keys().cloned().collect();
        providers.sort();
        providers.iter().filter_map(|p| self.peek(p)).collect()
    }

    /// Stores a snapshot as if read now; used by `/router` after a login and by tests.
    pub fn insert(&self, snapshot: UsageSnapshot) {
        self.lock()
            .cache
            .insert(snapshot.provider.clone(), snapshot);
        self.persist();
    }

    pub fn forget(&self, provider: &str) {
        self.lock().cache.remove(provider);
        self.persist();
    }

    fn persist(&self) {
        let Some(path) = &self.cache_file else { return };
        let snapshots: serde_json::Map<String, Value> = self
            .lock()
            .cache
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), serde_json::to_value(v).ok()?)))
            .collect();
        if let Err(err) = write_json(path, &json!({ "version": 1, "snapshots": snapshots })) {
            pi_ext_sdk::warn!("cannot write {}: {err}", path.display());
        }
    }

    fn gate(&self, provider: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = self.gates.lock().unwrap_or_else(|p| p.into_inner());
        gates.entry(provider.to_owned()).or_default().clone()
    }

    fn with_stale(&self, provider: &str, now: i64, message: String) -> UsageSnapshot {
        match self.lock().cache.get(provider) {
            Some(cached) => cached.stale(now, Some(message)),
            None => UsageSnapshot::error(provider, Status::Error, now, message),
        }
    }

    /// Current usage of `provider`; never fails. Concurrent reads of one provider share a gate.
    pub async fn get(
        &self,
        provider: &str,
        tokens: &dyn Tokens,
        options: GetOptions,
    ) -> UsageSnapshot {
        let gate = self.gate(provider);
        let _held = gate.lock().await;
        let now = now_ms();
        let cached = self.lock().cache.get(provider).cloned();
        if let Some(cached) = cached.filter(|c| !options.force && c.status == Status::Available) {
            if !options.fresh && cached.expires_at > now {
                return cached;
            }
            let reusable = options
                .not_before
                .is_none_or(|nb| cached.retrieved_at >= nb);
            if options.fresh && reusable && now - cached.retrieved_at < FRESH_REUSE_MS {
                return cached;
            }
        }
        let backoff = self.lock().backoff.get(provider).copied();
        if let Some((_, until)) = backoff.filter(|(_, until)| *until > now) {
            let message = format!(
                "Backing off after a failed usage request until {}.",
                iso_seconds(until)
            );
            return self.with_stale(provider, now, message);
        }
        if options.force {
            let last = self.lock().manual.get(provider).copied().unwrap_or(0);
            if now - last < MANUAL_REFRESH_MS {
                let message = "Manual refresh is limited to once per minute.".to_owned();
                return self.with_stale(provider, now, message);
            }
            self.lock().manual.insert(provider.to_owned(), now);
        }
        let refresh = options.force || options.fresh;
        let outcome = self.fetch(provider, tokens, now, refresh).await;
        let result = {
            let mut inner = self.lock();
            if outcome.snapshot.status == Status::Available {
                inner
                    .cache
                    .insert(provider.to_owned(), outcome.snapshot.clone());
                inner.backoff.remove(provider);
                outcome.snapshot
            } else {
                if outcome.failed {
                    let failures = inner.backoff.get(provider).map_or(0, |(n, _)| *n) + 1;
                    let step = FAILURE_BACKOFF_MS[failures.min(FAILURE_BACKOFF_MS.len()) - 1];
                    let delay = step
                        .max(outcome.retry_after.unwrap_or(0))
                        .min(RETRY_AFTER_CEILING_MS);
                    inner
                        .backoff
                        .insert(provider.to_owned(), (failures, now_ms() + delay));
                }
                match inner.cache.get(provider) {
                    Some(cached) => {
                        let stale = cached.stale(now_ms(), outcome.snapshot.message.clone());
                        inner.cache.insert(provider.to_owned(), stale.clone());
                        stale
                    }
                    None => outcome.snapshot,
                }
            }
        };
        self.persist();
        result
    }
}
