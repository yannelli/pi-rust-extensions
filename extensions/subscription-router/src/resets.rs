//! Banked usage resets of Codex accounts (the `rate-limit-reset-credits` endpoints of the Codex
//! CLI): list them, and spend one to refresh an account's five-hour and weekly windows.

use std::time::Duration;

use pi_ext_sdk::Host;
use rand::RngCore;
use serde::Deserialize;
use serde_json::json;

use crate::config::AccountPolicy;
use crate::family::Family;
use crate::fsutil::now_ms;
use crate::state::{HostTokens, State, lock};
use crate::usage::{GetOptions, Tokens, codex_account_id};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// One reset attempt per account in this window, so a failing request cannot drain them.
const ATTEMPT_GAP_MS: i64 = 10 * 60_000;
pub const RESETS_ENV: &str = "PI_ROUTER_CODEX_RESETS_URL";

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct ResetCredit {
    pub id: String,
    #[serde(default)]
    pub reset_type: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct ResetCredits {
    #[serde(default)]
    pub credits: Vec<ResetCredit>,
    #[serde(default)]
    pub available_count: i64,
}

/// What spending a reset did, as the backend reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Reset { windows: i64 },
    NothingToReset,
    NoCredit,
    AlreadyRedeemed,
    Other(String),
}

/// `<codex usage url without /usage>/rate-limit-reset-credits`, or `$PI_ROUTER_CODEX_RESETS_URL`.
pub fn resets_url(state: &State) -> String {
    if let Some(url) = std::env::var(RESETS_ENV).ok().filter(|u| !u.is_empty()) {
        return url;
    }
    let usage = state.usage().codex_url().trim_end_matches('/');
    let base = usage.strip_suffix("/usage").unwrap_or(usage);
    format!("{base}/rate-limit-reset-credits")
}

fn request(
    state: &State,
    method: reqwest::Method,
    url: &str,
    token: &str,
) -> reqwest::RequestBuilder {
    let request = state
        .http
        .request(method, url)
        .bearer_auth(token)
        .timeout(REQUEST_TIMEOUT);
    match codex_account_id(token) {
        Some(account) => request.header("ChatGPT-Account-Id", account),
        None => request,
    }
}

async fn token(host: &Host, state: &State, provider: &str) -> Result<String, String> {
    let tokens = HostTokens { host, state };
    tokens
        .token(provider)
        .await
        .filter(|t| !t.is_empty())
        .ok_or_else(|| format!("{provider} has no login"))
}

pub async fn list(host: &Host, state: &State, slot: u32) -> Result<ResetCredits, String> {
    let token = token(host, state, &Family::Codex.provider(slot)).await?;
    let response = request(state, reqwest::Method::GET, &resets_url(state), &token)
        .send()
        .await
        .map_err(|err| format!("cannot list usage resets: {err}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "listing usage resets failed: HTTP {}",
            response.status().as_u16()
        ));
    }
    response
        .json::<ResetCredits>()
        .await
        .map_err(|err| format!("unexpected usage reset list: {err}"))
}

/// A random version 4 UUID: the backend's idempotency key for one redeem.
fn request_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

pub async fn consume(host: &Host, state: &State, slot: u32) -> Result<Outcome, String> {
    let token = token(host, state, &Family::Codex.provider(slot)).await?;
    let url = format!("{}/consume", resets_url(state));
    let response = request(state, reqwest::Method::POST, &url, &token)
        .json(&json!({ "redeem_request_id": request_id() }))
        .send()
        .await
        .map_err(|err| format!("cannot use a usage reset: {err}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "using a usage reset failed: HTTP {}",
            response.status().as_u16()
        ));
    }
    #[derive(Deserialize)]
    struct Reply {
        code: String,
        #[serde(default)]
        windows_reset: i64,
    }
    let reply: Reply = response
        .json()
        .await
        .map_err(|err| format!("unexpected usage reset reply: {err}"))?;
    Ok(match reply.code.as_str() {
        "reset" => Outcome::Reset {
            windows: reply.windows_reset,
        },
        "nothing_to_reset" => Outcome::NothingToReset,
        "no_credit" => Outcome::NoCredit,
        "already_redeemed" => Outcome::AlreadyRedeemed,
        other => Outcome::Other(other.to_owned()),
    })
}

/// Whether a window full until `until` is worth a reset: it does not free up within the wait.
pub fn worth(policy: &AccountPolicy, until: i64, now: i64) -> bool {
    until - now > (policy.reset_min_wait_minutes * 60_000.0) as i64
}

/// Spends one banked reset of Codex account `slot` when it has one. Returns whether it reset.
pub async fn try_reset(host: &Host, state: &State, family: Family, slot: u32) -> bool {
    if family != Family::Codex {
        return false;
    }
    let now = now_ms();
    {
        let mut picks = lock(&state.picks);
        let last = picks.reset_tried.get(&(family, slot)).copied();
        if last.is_some_and(|at| now - at < ATTEMPT_GAP_MS) {
            return false;
        }
        picks.reset_tried.insert((family, slot), now);
    }
    let name = format!("{} #{slot}", family.short());
    let available = match list(host, state, slot).await {
        Ok(credits) => credits.available_count,
        Err(err) => {
            pi_ext_sdk::warn!("{name}: {err}");
            return false;
        }
    };
    if available <= 0 {
        return false;
    }
    let outcome = consume(host, state, slot).await;
    let provider = family.provider(slot);
    let tokens = HostTokens { host, state };
    let fresh = GetOptions {
        fresh: true,
        not_before: Some(now_ms()),
        ..Default::default()
    };
    match outcome {
        Ok(Outcome::Reset { .. }) => {
            lock(&state.picks).full_until.remove(&(family, slot));
            state.usage().get(&provider, &tokens, fresh).await;
            let left = available - 1;
            let notice =
                format!("subscription-router: used a usage reset on {name} ({left} left).");
            host.notify_info(notice).await.ok();
            true
        }
        Ok(outcome) => {
            pi_ext_sdk::warn!("{name}: usage reset not applied: {outcome:?}");
            state.usage().get(&provider, &tokens, fresh).await;
            false
        }
        Err(err) => {
            pi_ext_sdk::warn!("{name}: {err}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_are_version_4_uuids() {
        let id = request_id();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"), "{id}");
    }

    #[test]
    fn a_reset_is_worth_it_only_past_the_wait() {
        let policy = AccountPolicy::default();
        assert!(!worth(&policy, 30 * 60_000, 0));
        assert!(worth(&policy, 2 * 3_600_000, 0));
    }
}
