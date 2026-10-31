use serde::{Deserialize, Deserializer, Serialize};
use serde_json::json;

use super::{Host, without_nulls};
use crate::error::Result;

/// The subscription limits of one provider login, from `usage.get`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderUsage {
    #[serde(deserialize_with = "null_default")]
    pub provider: String,
    pub plan: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub windows: Vec<UsageWindow>,
    /// Set when the provider has no login, no usage fetcher, or the fetch failed.
    pub error: Option<String>,
    /// Unix milliseconds.
    #[serde(deserialize_with = "null_default")]
    pub fetched_at: u64,
    /// Codex credits.
    pub credits: Option<UsageCredits>,
    /// Why the fetch failed: `login`, `unsupported`, `rejected`, `rateLimited`, `http`, `network`,
    /// or `response`. Older hosts send no kind.
    pub error_kind: Option<String>,
    /// The `retry-after` of a failed request, in milliseconds from `fetched_at`.
    pub retry_after_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UsageCredits {
    pub available: bool,
    pub unlimited: bool,
    pub balance: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageWindow {
    #[serde(deserialize_with = "null_default")]
    pub label: String,
    /// 0 to 100.
    #[serde(deserialize_with = "null_default")]
    pub used_percent: f64,
    /// ISO 8601 UTC, for example `2026-10-30T17:00:00Z`.
    pub resets_at: Option<String>,
    /// `300` for 5 hours, `10080` for a week.
    pub window_minutes: Option<u64>,
    /// The exact window length in seconds, when the provider reports a whole number.
    pub window_seconds: Option<u64>,
    /// `provider` or `model`. Hosts before this field send none.
    pub scope: Option<String>,
    /// The model families of a `model` window, for example `opus`.
    #[serde(deserialize_with = "null_default")]
    pub families: Vec<String>,
    /// The provider's name of the window, for example `five_hour` or `primary_window`.
    pub id: Option<String>,
    /// The reset in Unix milliseconds, as exact as the provider reports it.
    pub resets_at_ms: Option<i64>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct UsageResult {
    #[serde(deserialize_with = "null_default")]
    providers: Vec<ProviderUsage>,
}

fn null_default<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// `usage.get`.
impl Host {
    /// Subscription limits of each logged-in provider, or of `provider` only.
    /// pi-rust 0.8.2 and older answer `-32601`.
    pub async fn usage(&self, provider: Option<&str>) -> Result<Vec<ProviderUsage>> {
        self.usage_with(provider, false).await
    }

    /// `refresh` skips pi's cache of 60 seconds (5 minutes for `google-antigravity`).
    pub async fn usage_with(
        &self,
        provider: Option<&str>,
        refresh: bool,
    ) -> Result<Vec<ProviderUsage>> {
        let params = json!({ "provider": provider, "refresh": refresh.then_some(true) });
        let result: Option<UsageResult> = self.call_as("usage.get", without_nulls(params)).await?;
        Ok(result.unwrap_or_default().providers)
    }
}
