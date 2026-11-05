//! Accounts behind one provider (pi with the `providerAuth` feature): every subscription account
//! logs in under its own id (`anthropic-2`), adds no models, and `before_provider_auth` picks the
//! account of each request by the provider's policy. A full account moves the next request to
//! another account, spends a banked reset, or falls back to a model of another provider.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, LazyLock};

use pi_ext_sdk::prelude::*;
use regex::Regex;

use crate::config::{AccountPolicy, AccountStrategy, ChainEntry, ModelRef, ResetUse, Strategy};
use crate::failover::switch_to;
use crate::family::{Family, parse_provider};
use crate::fsutil::{duration, now_ms};
use crate::selection::{QuotaScore, score_quota, select_candidate};
use crate::state::{HostTokens, State, current_model, lock, model_available};
use crate::usage::{GetOptions, Status, applies_to};

/// An account that failed without a known reset is skipped this long.
const UNKNOWN_RESET_MS: i64 = 5 * 60_000;

static HTTP_429: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b429\b").expect("valid regex"));
static QUOTA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)rate.?limit|usage.?limit|quota|too many requests|limit (?:reached|exceeded)|overloaded_error")
        .expect("valid regex")
});

/// Where a fallback came from, to go back when the provider has room again.
#[derive(Clone, Debug, PartialEq)]
pub struct FallbackFrom {
    pub family: Family,
    pub id: String,
    pub thinking: Option<String>,
    /// The fallback model, so a later choice of the user ends the fallback.
    pub to: ModelRef,
}

#[derive(Default)]
pub struct Picks {
    /// The account each provider uses now.
    pub current: HashMap<Family, u32>,
    /// `/router use`: the account of this session, by provider.
    pub pinned: HashMap<Family, u32>,
    /// The account a router profile chose for its routed model.
    pub routed: Option<(Family, String, u32)>,
    /// The provider, account, and model of the last request.
    pub last: Option<(Family, u32, String)>,
    /// Accounts that hit their limit, until when.
    pub full_until: HashMap<(Family, u32), i64>,
    pub fallback_from: Option<FallbackFrom>,
    /// When a usage reset was last tried per account.
    pub reset_tried: HashMap<(Family, u32), i64>,
}

pub fn enabled(state: &State) -> bool {
    state.accounts_mode.load(Ordering::Relaxed)
}

/// Account numbers of `family`: 1 (pi's own login) and the stored ones, ascending.
pub fn account_ids(state: &State, family: Family) -> Vec<u32> {
    let mut ids = vec![1];
    ids.extend(
        state
            .accounts
            .list()
            .into_iter()
            .filter(|a| a.family == family)
            .map(|a| a.slot),
    );
    ids.sort_unstable();
    ids.dedup();
    ids
}

pub fn account_name(family: Family, slot: u32) -> String {
    format!("{} #{slot}", family.short())
}

/// When account `slot` frees up for model `id`, if it is full now: a recent limit error, a
/// confirmed block, or a window of the cached usage at `full_at` percent with its reset ahead.
pub fn full_until(
    state: &State,
    family: Family,
    slot: u32,
    id: &str,
    full_at: f64,
    now: i64,
) -> Option<i64> {
    let failed = lock(&state.picks).full_until.get(&(family, slot)).copied();
    if let Some(until) = failed.filter(|until| *until > now) {
        return Some(until);
    }
    let provider = family.provider(slot);
    let model = ModelRef::new(&provider, id);
    let blocked = state.router.tracker().active(now);
    if let Some(mark) = blocked.iter().find(|mark| mark.covers(&model)) {
        return Some(mark.until);
    }
    let snapshot = state.usage().peek(&provider)?;
    if !matches!(snapshot.status, Status::Available | Status::Stale) {
        return None;
    }
    if snapshot.credits.as_ref().is_some_and(|c| c.unlimited) {
        return None;
    }
    snapshot
        .windows
        .iter()
        .filter(|w| applies_to(w, id) && w.used_percent >= full_at)
        .filter(|w| {
            w.reset_at
                .map_or(snapshot.status == Status::Available, |r| r > now)
        })
        .map(|w| w.reset_at.unwrap_or(now + UNKNOWN_RESET_MS))
        .max()
}

pub fn all_full(state: &State, family: Family, id: &str, now: i64) -> bool {
    let full_at = state.config().policy(family).full_at();
    account_ids(state, family)
        .into_iter()
        .all(|slot| full_until(state, family, slot, id, full_at, now).is_some())
}

/// The account for the next request of model `id`: a routed or pinned account, else the policy.
pub fn choose(state: &State, family: Family, id: &str, now: i64) -> u32 {
    let ids = account_ids(state, family);
    let policy = state.config().policy(family);
    let (routed, pinned, current) = {
        let picks = lock(&state.picks);
        let routed = picks
            .routed
            .clone()
            .filter(|(f, model, slot)| *f == family && model == id && ids.contains(slot))
            .map(|(_, _, slot)| slot);
        let pinned = picks
            .pinned
            .get(&family)
            .copied()
            .filter(|s| ids.contains(s));
        let current = picks
            .current
            .get(&family)
            .copied()
            .filter(|s| ids.contains(s));
        (routed, pinned, current)
    };
    let chosen = routed
        .or(pinned)
        .unwrap_or_else(|| by_policy(state, family, id, &policy, &ids, current, now));
    lock(&state.picks).current.insert(family, chosen);
    chosen
}

fn by_policy(
    state: &State,
    family: Family,
    id: &str,
    policy: &AccountPolicy,
    ids: &[u32],
    current: Option<u32>,
    now: i64,
) -> u32 {
    let full_at = policy.full_at();
    let free = |slot: u32| full_until(state, family, slot, id, full_at, now).is_none();
    match policy.strategy {
        AccountStrategy::Manual if ids.contains(&policy.account) => policy.account,
        AccountStrategy::Manual => 1,
        AccountStrategy::Fill | AccountStrategy::Threshold => {
            let start = current.unwrap_or(ids[0]);
            if free(start) {
                return start;
            }
            let at = ids.iter().position(|s| *s == start).unwrap_or(0);
            (1..ids.len())
                .map(|step| ids[(at + step) % ids.len()])
                .find(|slot| free(*slot))
                .unwrap_or(start)
        }
        AccountStrategy::Balance => {
            let open: Vec<u32> = ids.iter().copied().filter(|s| free(*s)).collect();
            if open.is_empty() {
                return current.unwrap_or(ids[0]);
            }
            let scores: Vec<QuotaScore> = open
                .iter()
                .map(|slot| {
                    let provider = family.provider(*slot);
                    let model = ModelRef::new(&provider, id);
                    score_quota(&model, state.usage().peek(&provider).as_ref(), now)
                })
                .collect();
            let incumbent = current.and_then(|c| open.iter().position(|s| *s == c));
            select_candidate(&scores, Strategy::Balanced, incumbent)
                .map_or(open[0], |index| open[index])
        }
    }
}

/// `before_provider_auth`: names the account of a request for a subscription model.
pub async fn on_auth(host: Host, state: Arc<State>, event: Value) -> Result<EventReply> {
    if !enabled(&state) {
        return Ok(EventReply::none());
    }
    let provider = event
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let id = event
        .pointer("/model/id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some((family, 1)) = parse_provider(provider) else {
        return Ok(EventReply::none());
    };
    let slot = choose(&state, family, id, now_ms());
    let previous = lock(&state.picks)
        .last
        .replace((family, slot, id.to_owned()));
    if previous.is_some_and(|(f, s, _)| f == family && s != slot) {
        crate::status::update(&host, &state).await;
    }
    Ok(if slot > 1 {
        EventReply::result(json!({ "authProvider": family.provider(slot) }))
    } else {
        EventReply::none()
    })
}

/// The failed assistant message of a limit error on a subscription model, with its model id.
fn limit_error(message: &Value, status: Option<u64>) -> Option<(Family, String)> {
    let get = |k: &str| message.get(k).and_then(Value::as_str);
    if get("role") != Some("assistant") || get("stopReason") != Some("error") {
        return None;
    }
    let error = get("errorMessage").unwrap_or_default();
    if status != Some(429) && !HTTP_429.is_match(error) && !QUOTA.is_match(error) {
        return None;
    }
    let (family, slot) = parse_provider(get("provider")?)?;
    (slot == 1).then(|| (family, get("model").unwrap_or_default().to_owned()))
}

/// A retryable error for pi's retry: `HTTP 429` without the words pi treats as final.
fn retry_message(message: &Value, reason: &str) -> Value {
    let mut message = message.clone();
    message["errorMessage"] = json!(format!("HTTP 429: rate limited; {reason}"));
    message
}

fn exhausted_message(message: &Value, family: Family, until: Option<i64>, now: i64) -> Value {
    let mut message = message.clone();
    let reset = until.map_or(String::new(), |u| {
        format!(" The first one frees up in {}.", duration(u - now))
    });
    message["errorMessage"] = json!(format!(
        "{} quota exceeded: every account is at its limit.{reset} Add a fallback model or allow usage resets in /router policy.",
        family.label()
    ));
    message
}

/// Reads the usage of the account now, to learn when it frees up.
async fn confirm_full(
    host: &Host,
    state: &State,
    family: Family,
    slot: u32,
    id: &str,
) -> Option<i64> {
    let provider = family.provider(slot);
    let tokens = HostTokens { host, state };
    let options = GetOptions {
        fresh: true,
        not_before: Some(now_ms()),
        ..Default::default()
    };
    state.usage().get(&provider, &tokens, options).await;
    full_until(state, family, slot, id, 100.0, now_ms())
}

/// `message_end`: a limit error moves the retry to another account, a reset, or a fallback model.
pub async fn on_message_end(host: Host, state: Arc<State>, event: Value) -> Result<EventReply> {
    let status = {
        let mut failover = lock(&state.failover);
        failover.request_provider.take();
        failover.status.take()
    };
    let message = event.get("message").cloned().unwrap_or(Value::Null);
    let Some((family, id)) = limit_error(&message, status) else {
        return Ok(EventReply::none());
    };
    if !state.config().auto_switch_on_429 {
        return Ok(EventReply::none());
    }
    let slot = match lock(&state.picks).last.clone() {
        Some((f, slot, last_id)) if f == family && last_id == id => slot,
        _ => 1,
    };
    let now = now_ms();
    let until = confirm_full(&host, &state, family, slot, &id)
        .await
        .unwrap_or(now + UNKNOWN_RESET_MS);
    lock(&state.picks).full_until.insert((family, slot), until);
    let policy = state.config().policy(family);
    let failed = account_name(family, slot);
    if policy.use_resets == ResetUse::WhenFull
        && crate::resets::worth(&policy, until, now)
        && crate::resets::try_reset(&host, &state, family, slot).await
    {
        return Ok(EventReply::result(json!({
            "message": retry_message(&message, &format!("retrying {failed} after a usage reset"))
        })));
    }
    lock(&state.picks).pinned.remove(&family);
    let next = choose(&state, family, &id, now);
    if next != slot && full_until(&state, family, next, &id, policy.full_at(), now).is_none() {
        let notice = format!(
            "{failed} is at its limit (frees up in {}). Retrying on {}.",
            duration(until - now),
            account_name(family, next)
        );
        host.notify_warning(notice).await.ok();
        crate::status::update(&host, &state).await;
        let reason = format!("retrying on {}", account_name(family, next));
        return Ok(EventReply::result(
            json!({ "message": retry_message(&message, &reason) }),
        ));
    }
    if let Some(reason) = when_all_full(&host, &state, family, &id).await {
        return Ok(EventReply::result(
            json!({ "message": retry_message(&message, &reason) }),
        ));
    }
    let first_free = account_ids(&state, family)
        .into_iter()
        .filter_map(|s| full_until(&state, family, s, &id, policy.full_at(), now))
        .min();
    Ok(EventReply::result(json!({
        "message": exhausted_message(&message, family, first_free, now)
    })))
}

/// Every account of `family` is full: spend a reset (`when-all-full`), else fall back to another
/// provider's model. Returns what it did, for the retry message and the notice.
async fn when_all_full(
    host: &Host,
    state: &Arc<State>,
    family: Family,
    id: &str,
) -> Option<String> {
    let policy = state.config().policy(family);
    let now = now_ms();
    if policy.use_resets == ResetUse::WhenAllFull {
        let mut accounts: Vec<(u32, i64)> = account_ids(state, family)
            .into_iter()
            .filter_map(|s| Some((s, full_until(state, family, s, id, 100.0, now)?)))
            .filter(|(_, until)| crate::resets::worth(&policy, *until, now))
            .collect();
        // The account that stays full longest gains the most from a reset.
        accounts.sort_by_key(|(_, until)| std::cmp::Reverse(*until));
        for (slot, _) in accounts {
            if crate::resets::try_reset(host, state, family, slot).await {
                lock(&state.picks).pinned.insert(family, slot);
                return Some(format!(
                    "retrying {} after a usage reset",
                    account_name(family, slot)
                ));
            }
        }
    }
    let thinking = host.thinking_level().await.ok().flatten();
    for entry in &policy.fallback {
        if let Some(to) = fall_back(host, state, entry).await {
            lock(&state.picks).fallback_from = Some(FallbackFrom {
                family,
                id: id.to_owned(),
                thinking: thinking.clone(),
                to: to.clone(),
            });
            let back = if policy.return_when_available {
                " It switches back when an account has room."
            } else {
                ""
            };
            let notice = format!(
                "Every {} account is at its limit. Using {} for now.{back}",
                family.label(),
                to.key()
            );
            host.notify_warning(notice).await.ok();
            crate::status::update(host, state).await;
            return Some(format!("falling back to {}", to.key()));
        }
    }
    None
}

/// Switches the session to fallback `entry` when it is usable; returns the model pi now uses.
async fn fall_back(host: &Host, state: &Arc<State>, entry: &ChainEntry) -> Option<ModelRef> {
    let now = now_ms();
    let (target, account) = match parse_provider(&entry.model.provider) {
        Some((family, slot)) => (
            ModelRef::new(family.base(), &entry.model.id),
            Some((family, slot)),
        ),
        None => (entry.model.clone(), None),
    };
    if !model_available(host, &target).await {
        return None;
    }
    if let Some((family, slot)) = account {
        if !account_ids(state, family).contains(&slot) {
            return None;
        }
        let full_at = state.config().policy(family).full_at();
        if full_until(state, family, slot, &target.id, full_at, now).is_some()
            && all_full(state, family, &target.id, now)
        {
            return None;
        }
        if slot > 1 {
            lock(&state.picks).pinned.insert(family, slot);
        }
    }
    if !switch_to(host, state, &target.provider, &target.id).await {
        return None;
    }
    if let Some(level) = &entry.thinking {
        host.set_thinking_level(level).await.ok();
    }
    Some(target)
}

/// `before_agent_start`: moves a session off a retired account model, returns from a fallback
/// when the provider has room, and falls back before sending when every account is full.
pub async fn on_prompt_start(host: &Host, state: &Arc<State>) {
    let Some(current) = current_model(host) else {
        return;
    };
    let now = now_ms();
    if let Some((family, slot)) = parse_provider(&current.provider).filter(|(_, slot)| *slot > 1) {
        if switch_to(host, state, family.base(), &current.id).await {
            lock(&state.picks).pinned.insert(family, slot);
            let notice = format!(
                "{} is now an account of {}: this session uses {}/{} on {}. /router use changes the account.",
                current.provider,
                family.base(),
                family.base(),
                current.id,
                account_name(family, slot)
            );
            host.notify_info(notice).await.ok();
            crate::status::update(host, state).await;
        }
        return;
    }
    let from = lock(&state.picks).fallback_from.clone();
    if let Some(from) = from {
        if current != from.to {
            lock(&state.picks).fallback_from = None;
        } else if state.config().policy(from.family).return_when_available
            && !all_full(state, from.family, &from.id, now)
            && switch_to(host, state, from.family.base(), &from.id).await
        {
            if let Some(level) = &from.thinking {
                host.set_thinking_level(level).await.ok();
            }
            lock(&state.picks).fallback_from = None;
            let notice = format!(
                "{} has room again. Back on {}/{}.",
                from.family.label(),
                from.family.base(),
                from.id
            );
            host.notify_info(notice).await.ok();
            crate::status::update(host, state).await;
            return;
        }
    }
    if let Some((family, 1)) = parse_provider(&current.provider) {
        if all_full(state, family, &current.id, now) {
            when_all_full(host, state, family, &current.id).await;
        }
    }
}

/// `model_select` by the user ends a fallback.
pub fn user_selected(state: &State) {
    lock(&state.picks).fallback_from = None;
}

/// The status line of the account a subscription model uses now.
pub fn status_slot(state: &State, family: Family) -> u32 {
    lock(&state.picks)
        .current
        .get(&family)
        .copied()
        .unwrap_or(1)
}

/// Snapshot of usage use for `/router` output: `(slot, full until)` per account.
pub fn account_states(
    state: &State,
    family: Family,
    id: &str,
    now: i64,
) -> Vec<(u32, Option<i64>)> {
    let full_at = state.config().policy(family).full_at();
    account_ids(state, family)
        .into_iter()
        .map(|slot| (slot, full_until(state, family, slot, id, full_at, now)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_errors_of_subscription_models_only() {
        let message = |provider: &str, error: &str| json!({ "role": "assistant", "stopReason": "error", "provider": provider, "model": "m", "errorMessage": error });
        assert!(limit_error(&message("anthropic", "HTTP 429 too many"), None).is_some());
        assert!(
            limit_error(
                &message("openai-codex", "You've hit your usage limit"),
                None
            )
            .is_some()
        );
        assert!(limit_error(&message("anthropic", "overloaded"), Some(429)).is_some());
        assert!(limit_error(&message("anthropic", "invalid request"), None).is_none());
        assert!(limit_error(&message("anthropic-2", "HTTP 429"), None).is_none());
        assert!(limit_error(&message("openai", "HTTP 429"), None).is_none());
    }

    #[test]
    fn retry_messages_avoid_words_that_stop_retries() {
        let message = retry_message(
            &json!({ "errorMessage": "quota exceeded" }),
            "retrying on Claude #2",
        );
        let text = message["errorMessage"].as_str().unwrap();
        assert!(text.starts_with("HTTP 429"));
        assert!(!text.contains("quota"));
    }
}
