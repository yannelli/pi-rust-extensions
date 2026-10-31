//! Usage through pi's `usage.get`: pi reads the endpoint with its own login, also for slots that
//! log in with `oauth: { builtin }`. The router keeps its cache, backoff, and window names.

use pi_ext_sdk::ProviderUsage;

use super::service::Outcome;
use super::{
    CACHE_TTL_MS, Credits, FIVE_HOURS_MS, SEVEN_DAYS_MS, Scope, Status, UsageSnapshot, UsageWindow,
};
use crate::family::Family;

/// What `usage.get` gave for one provider.
pub enum CoreRead {
    /// Read the endpoint here: pi has no `usage.get`, is older than its slot and scope support,
    /// or the slot keeps the router's own token.
    Unsupported,
    Usage(ProviderUsage),
    /// The request to pi failed.
    Failed(String),
}

/// The windows the router has always read; pi also reports extra Codex limits and other models.
const ANTHROPIC_WINDOWS: [&str; 4] = [
    "five_hour",
    "seven_day",
    "seven_day_opus",
    "seven_day_sonnet",
];
const CODEX_WINDOWS: [&str; 2] = ["primary_window", "secondary_window"];

/// A result in the format of hosts before slot aliases and window scopes, which the router
/// reads itself instead.
pub fn from_older_host(usage: &ProviderUsage) -> bool {
    let unaliased_slot = usage.error_kind.is_none()
        && usage
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("pi has no usage report for"));
    let unscoped = usage.windows.iter().any(|window| window.scope.is_none());
    let unkinded_failure = usage.error.is_some() && usage.error_kind.is_none();
    unaliased_slot || unscoped || unkinded_failure
}

fn codex_name(minutes: Option<u64>, id: &str) -> (String, i64, &'static str) {
    let (fallback, fallback_ms) = if id == "primary_window" {
        ("5h", FIVE_HOURS_MS)
    } else {
        ("7d", SEVEN_DAYS_MS)
    };
    match minutes {
        Some(10_080) => ("7d".into(), SEVEN_DAYS_MS, "reported"),
        Some(300) => ("5h".into(), FIVE_HOURS_MS, "reported"),
        Some(minutes) => (
            format!("{}s", minutes * 60),
            minutes as i64 * 60_000,
            "reported",
        ),
        None => (fallback.into(), fallback_ms, "assumed"),
    }
}

fn window(family: Family, core: &pi_ext_sdk::UsageWindow) -> Option<UsageWindow> {
    let id = core.id.as_deref()?;
    let (name, duration_ms, span) = match family {
        Family::Anthropic if ANTHROPIC_WINDOWS.contains(&id) => {
            let duration = if id == "five_hour" {
                FIVE_HOURS_MS
            } else {
                SEVEN_DAYS_MS
            };
            (id.to_owned(), duration, "reported")
        }
        Family::Codex if CODEX_WINDOWS.contains(&id) => codex_name(core.window_minutes, id),
        _ => return None,
    };
    let model = core.scope.as_deref() == Some("model");
    Some(UsageWindow {
        name,
        used_percent: core.used_percent.clamp(0.0, 100.0),
        reset_at: core.resets_at_ms,
        duration_ms: Some(duration_ms),
        span_source: Some(span.into()),
        scope: if model { Scope::Model } else { Scope::Provider },
        families: model.then(|| core.families.clone()),
    })
}

/// The router's snapshot of a `usage.get` entry. `now` stamps failures; reads keep pi's time.
pub fn outcome(provider: &str, family: Family, usage: &ProviderUsage, now: i64) -> Outcome {
    if let Some(error) = &usage.error {
        let login = matches!(usage.error_kind.as_deref(), Some("login" | "unsupported"));
        let status = if login {
            Status::Unavailable
        } else {
            Status::Error
        };
        let retry_after = usage.retry_after_ms;
        return Outcome {
            snapshot: UsageSnapshot::error(provider, status, now, error),
            failed: !login,
            retry_after,
        };
    }
    let windows: Vec<UsageWindow> = usage
        .windows
        .iter()
        .filter_map(|core| window(family, core))
        .collect();
    if windows.is_empty() {
        let snapshot = UsageSnapshot::error(
            provider,
            Status::Error,
            now,
            "Usage response had an unsupported shape.",
        );
        return Outcome {
            snapshot,
            failed: true,
            retry_after: None,
        };
    }
    let retrieved_at = if usage.fetched_at > 0 {
        usage.fetched_at as i64
    } else {
        now
    };
    let credits = usage.credits.as_ref().map(|credits| Credits {
        available: credits.available,
        unlimited: credits.unlimited,
        balance: credits.balance.clone(),
    });
    let snapshot = UsageSnapshot {
        provider: provider.to_owned(),
        status: Status::Available,
        windows,
        credits,
        tier: usage.plan.clone(),
        retrieved_at,
        expires_at: retrieved_at + CACHE_TTL_MS,
        message: None,
    };
    Outcome {
        snapshot,
        failed: false,
        retry_after: None,
    }
}
