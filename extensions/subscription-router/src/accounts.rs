//! Credentials of extension-owned slots (`openai-codex-<n>`, `anthropic-<n>`, n >= 2) in
//! `<agentDir>/ext/subscription-router/accounts.json`, mode 0600.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::family::{Family, parse_provider};
use crate::fsutil::write_json;
use crate::oauth::Credential;

/// Refresh this long before the access token expires.
pub const REFRESH_MARGIN_MS: i64 = 5 * 60_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    #[serde(rename = "type")]
    kind: String,
    access: String,
    refresh: String,
    expires: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub provider: String,
    pub family: Family,
    pub slot: u32,
    pub credential: Credential,
}

impl Account {
    pub fn new(family: Family, slot: u32, credential: Credential) -> Self {
        Self {
            provider: family.provider(slot),
            family,
            slot,
            credential,
        }
    }

    pub fn needs_refresh(&self, now: i64) -> bool {
        self.credential.expires - now < REFRESH_MARGIN_MS
    }
}

pub struct AccountStore {
    path: PathBuf,
    accounts: Mutex<Vec<Account>>,
}

impl AccountStore {
    /// Reads the file; a missing or unreadable file starts empty.
    pub fn load(path: PathBuf) -> Self {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let raw: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let stored: BTreeMap<String, Stored> =
            serde_json::from_value(raw["accounts"].clone()).unwrap_or_default();
        let mut accounts: Vec<Account> = stored
            .into_iter()
            .filter_map(|(provider, s)| {
                let (family, slot) = parse_provider(&provider).filter(|(_, slot)| *slot >= 2)?;
                let credential = Credential {
                    access: s.access,
                    refresh: s.refresh,
                    expires: s.expires,
                    account_id: s.account_id,
                };
                Some(Account {
                    provider,
                    family,
                    slot,
                    credential,
                })
            })
            .collect();
        accounts.sort_by_key(|a| (a.family.base(), a.slot));
        Self {
            path,
            accounts: Mutex::new(accounts),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Account>> {
        self.accounts.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn list(&self) -> Vec<Account> {
        self.lock().clone()
    }

    pub fn get(&self, provider: &str) -> Option<Account> {
        self.lock().iter().find(|a| a.provider == provider).cloned()
    }

    /// The lowest free slot number of `family`, starting at 2.
    pub fn next_slot(&self, family: Family) -> u32 {
        let accounts = self.lock();
        (2..)
            .find(|n| !accounts.iter().any(|a| a.family == family && a.slot == *n))
            .unwrap_or(2)
    }

    pub fn upsert(&self, account: Account) -> std::io::Result<()> {
        {
            let mut accounts = self.lock();
            accounts.retain(|a| a.provider != account.provider);
            accounts.push(account);
            accounts.sort_by_key(|a| (a.family.base(), a.slot));
        }
        self.save()
    }

    pub fn remove(&self, provider: &str) -> std::io::Result<bool> {
        let removed = {
            let mut accounts = self.lock();
            let before = accounts.len();
            accounts.retain(|a| a.provider != provider);
            accounts.len() != before
        };
        self.save()?;
        Ok(removed)
    }

    fn save(&self) -> std::io::Result<()> {
        let accounts: BTreeMap<String, Stored> = self
            .lock()
            .iter()
            .map(|a| {
                let c = &a.credential;
                let stored = Stored {
                    kind: "oauth".into(),
                    access: c.access.clone(),
                    refresh: c.refresh.clone(),
                    expires: c.expires,
                    account_id: c.account_id.clone(),
                };
                (a.provider.clone(), stored)
            })
            .collect();
        write_json(&self.path, &json!({ "version": 1, "accounts": accounts }))
    }
}
