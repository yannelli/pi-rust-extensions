//! One network read of a usage endpoint.

use serde_json::Value;

use super::service::{Outcome, REQUEST_TIMEOUT, Tokens, UsageService};
use super::{CACHE_TTL_MS, Status, UsageSnapshot, parse_anthropic, parse_codex};
use crate::family::{Family, parse_provider};
use crate::fsutil::now_ms;

const ANTHROPIC_BETA: &str = "oauth-2025-04-20";
const ANTHROPIC_USER_AGENT: &str = "claude-code/2.1";

impl UsageService {
    pub(super) async fn fetch(&self, provider: &str, tokens: &dyn Tokens, now: i64) -> Outcome {
        let unfetched = |status, message: &str| Outcome {
            snapshot: UsageSnapshot::error(provider, status, now, message),
            failed: false,
            retry_after: None,
        };
        let failure = |message: String, retry_after| Outcome {
            snapshot: UsageSnapshot::error(provider, Status::Error, now, message),
            failed: true,
            retry_after,
        };
        let Some((family, _)) = parse_provider(provider) else {
            return unfetched(
                Status::Unavailable,
                "No subscription usage collector exists for this provider.",
            );
        };
        let token = match tokio::time::timeout(REQUEST_TIMEOUT, tokens.token(provider)).await {
            Ok(Some(token)) if !token.is_empty() => token,
            Ok(_) => {
                return unfetched(
                    Status::Unavailable,
                    "No OAuth credential is available for this provider.",
                );
            }
            Err(_) => return failure("OAuth credential lookup timed out.".into(), None),
        };
        if family == Family::Anthropic && token.starts_with("sk-ant-api") {
            return unfetched(
                Status::Unavailable,
                "API-key auth has no subscription usage window.",
            );
        }
        let request = match family {
            Family::Codex => {
                let request = self.client.get(&self.urls.codex).bearer_auth(&token);
                match super::codex_account_id(&token) {
                    Some(account) => request.header("ChatGPT-Account-Id", account),
                    None => request,
                }
            }
            Family::Anthropic => self
                .client
                .get(&self.urls.anthropic)
                .bearer_auth(&token)
                .header("anthropic-beta", ANTHROPIC_BETA)
                .header("User-Agent", ANTHROPIC_USER_AGENT),
        };
        let response = match request.timeout(REQUEST_TIMEOUT).send().await {
            Ok(response) => response,
            Err(err) if err.is_timeout() => {
                return failure("Usage request timed out.".into(), None);
            }
            Err(_) => return failure("Usage request failed.".into(), None),
        };
        let status = response.status();
        if !status.is_success() {
            let header = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok());
            let retry_after = header.and_then(|h| retry_after_ms(h, now_ms()));
            return failure(
                format!("Usage request failed ({}).", status.as_u16()),
                retry_after,
            );
        }
        let payload: Value = match response.json().await {
            Ok(payload) => payload,
            Err(_) => return failure("Usage response had an unsupported shape.".into(), None),
        };
        let parsed = match family {
            Family::Codex => parse_codex(&payload, now),
            Family::Anthropic => parse_anthropic(&payload),
        };
        let Some(parsed) = parsed else {
            return failure("Usage response had an unsupported shape.".into(), None);
        };
        let snapshot = UsageSnapshot {
            provider: provider.to_owned(),
            status: Status::Available,
            windows: parsed.windows,
            credits: parsed.credits,
            tier: parsed.tier,
            retrieved_at: now,
            expires_at: now + CACHE_TTL_MS,
            message: None,
        };
        Outcome {
            snapshot,
            failed: false,
            retry_after: None,
        }
    }
}

/// `Retry-After` as milliseconds from `now`: delta-seconds or an HTTP-date.
pub fn retry_after_ms(header: &str, now: i64) -> Option<i64> {
    let trimmed = header.trim();
    let ms = match trimmed.parse::<i64>() {
        Ok(seconds) => seconds.saturating_mul(1000),
        Err(_) => {
            chrono::DateTime::parse_from_rfc2822(trimmed)
                .ok()?
                .timestamp_millis()
                - now
        }
    };
    (ms > 0).then_some(ms)
}
