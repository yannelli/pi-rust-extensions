//! The `usage` and `usageReset` segments: subscription limits of the current model's provider
//! from `usage.get`. A process-wide cache holds them; [`start`] refreshes it in the background.

mod format;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pi_ext_sdk::{Host, ProviderUsage, code};
use serde_json::Value;

pub use format::{format_duration, format_percent, format_usage, parse_utc_ms, role, short_label};

use crate::themes::Role;

const PROVIDERS: &[&str] = &[
    "anthropic",
    "openai-codex",
    "openai",
    "cursor",
    "google-antigravity",
];
const TICK: Duration = Duration::from_secs(2);
const REFRESH: Duration = Duration::from_secs(90);
const AFTER_RUN: Duration = Duration::from_secs(5);
/// The footer renders at least every 30 seconds, so a configured segment asks more often.
const WANTED_FOR: Duration = Duration::from_secs(120);

#[derive(Default)]
struct Cache {
    usage: HashMap<String, ProviderUsage>,
    attempts: HashMap<String, Instant>,
    wanted: Option<(String, Instant)>,
    run_ended: bool,
}

fn cache() -> MutexGuard<'static, Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The `usage.get` provider of a model provider; `None` for providers without usage data.
pub fn usage_provider(model_provider: &str) -> Option<&'static str> {
    PROVIDERS
        .iter()
        .copied()
        .find(|name| *name == model_provider)
}

/// The cached usage of a model provider. Marks the provider as wanted, so the refresh task
/// fetches it.
pub fn lookup(model_provider: &str) -> Option<ProviderUsage> {
    let provider = usage_provider(model_provider)?;
    let mut cache = cache();
    cache.wanted = Some((provider.to_string(), Instant::now()));
    cache.usage.get(provider).cloned()
}

pub fn store(usage: ProviderUsage) {
    cache().usage.insert(usage.provider.clone(), usage);
}

/// Text of the `usage` segment, or of `usageReset` when `details` is set.
pub fn segment_text(model_provider: &str, details: bool) -> Option<String> {
    format_usage(&lookup(model_provider)?, details, now_ms())
}

pub fn segment_role(model_provider: Option<&str>) -> Role {
    model_provider
        .and_then(lookup)
        .map_or(Role::ContextOk, |usage| role(&usage))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// Starts the refresh task once per process. It sends nothing until a usage segment asks for a
/// provider, then fetches it every 90 seconds and after each run; it stops without `usage.get`.
pub fn start(host: &Host) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if !STARTED.swap(true, Ordering::SeqCst) {
        tokio::spawn(watch(host.clone()));
    }
}

async fn watch(host: Host) {
    let mut working = false;
    loop {
        tokio::time::sleep(TICK).await;
        let run_state = host.snapshot().and_then(|snapshot| {
            let footer = snapshot.footer.as_ref()?;
            footer
                .get("runState")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        let now_working = matches!(run_state.as_deref(), Some("working" | "compacting"));
        if working && !now_working {
            cache().run_ended = true;
        }
        working = now_working;
        let Some(provider) = due(Instant::now()) else {
            continue;
        };
        if !fetch(&host, &provider).await {
            return;
        }
    }
}

fn due(now: Instant) -> Option<String> {
    let mut cache = cache();
    let (provider, asked) = cache.wanted.clone()?;
    if now.duration_since(asked) > WANTED_FOR {
        return None;
    }
    let since = cache
        .attempts
        .get(&provider)
        .map(|at| now.duration_since(*at));
    let run_ended = cache.run_ended;
    let due = since.is_none_or(|since| since >= REFRESH || (run_ended && since >= AFTER_RUN));
    if !due {
        return None;
    }
    cache.run_ended = false;
    cache.attempts.insert(provider.clone(), now);
    Some(provider)
}

/// Fetches `provider` into the cache. `false` when pi has no `usage.get`; other errors keep
/// the cached data.
pub async fn fetch(host: &Host, provider: &str) -> bool {
    match host.usage(Some(provider)).await {
        Ok(list) => {
            if let Some(usage) = list.into_iter().find(|usage| usage.provider == provider) {
                store(usage);
            }
            true
        }
        Err(error) => error.code() != code::METHOD_NOT_FOUND,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_providers_have_usage() {
        assert_eq!(usage_provider("openai-codex"), Some("openai-codex"));
        assert_eq!(usage_provider("openrouter"), None);
        assert_eq!(lookup("openrouter"), None);
        assert_eq!(segment_text("openrouter", false), None);
        assert_eq!(segment_role(Some("openrouter")), Role::ContextOk);
    }

    #[test]
    fn cached_errors_hide_the_segment() {
        store(ProviderUsage {
            provider: "cursor".into(),
            error: Some("not logged in".into()),
            ..ProviderUsage::default()
        });
        assert_eq!(segment_text("cursor", false), None);
        assert_eq!(segment_text("cursor", true), None);
    }
}
