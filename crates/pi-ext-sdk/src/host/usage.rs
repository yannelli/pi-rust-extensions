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
