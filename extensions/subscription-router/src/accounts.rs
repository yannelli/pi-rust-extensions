//! Credentials of extension-owned slots (`openai-codex-<n>`, `anthropic-<n>`, n >= 2) in
//! `<agentDir>/ext/subscription-router/accounts.json`, mode 0600.

use std::future::Future;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::family::{Family, parse_provider};
use crate::filelock::FileLock;
use crate::fsutil::{now_ms, write_json};
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

/// The `accounts` object of the file; entries that fail to parse stay in it untouched.
type Entries = serde_json::Map<String, Value>;

fn invalid(path: &Path, detail: impl std::fmt::Display) -> std::io::Error {
    let message = format!(
        "{} is unreadable ({detail}); fix or remove it before changing accounts",
        path.display()
    );
    std::io::Error::new(ErrorKind::InvalidData, message)
}

/// A missing file has no entries; an unreadable or malformed file is an error.
fn read_entries(path: &Path) -> std::io::Result<Entries> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Entries::new()),
        Err(error) => return Err(invalid(path, error)),
    };
    let raw: Value = serde_json::from_str(&text).map_err(|error| invalid(path, error))?;
    match raw.get("accounts") {
        _ if !raw.is_object() => Err(invalid(path, "not a JSON object")),
        None => Ok(Entries::new()),
        Some(Value::Object(entries)) => Ok(entries.clone()),
        Some(_) => Err(invalid(path, "`accounts` is not an object")),
    }
}

fn parse_entry(provider: &str, value: &Value) -> Option<Account> {
    let (family, slot) = parse_provider(provider).filter(|(_, slot)| *slot >= 2)?;
    let s = Stored::deserialize(value).ok()?;
    let credential = Credential {
        access: s.access,
        refresh: s.refresh,
        expires: s.expires,
        account_id: s.account_id,
    };
    Some(Account {
        provider: provider.to_owned(),
        family,
        slot,
        credential,
    })
}

/// The valid accounts and the keys of entries that are not.
fn parse_entries(entries: &Entries) -> (Vec<Account>, Vec<String>) {
    let mut bad = Vec::new();
    let mut accounts: Vec<Account> = entries
        .iter()
        .filter_map(|(provider, value)| {
            let account = parse_entry(provider, value);
            if account.is_none() {
                bad.push(provider.clone());
            }
            account
        })
        .collect();
    accounts.sort_by_key(|a| (a.family.base(), a.slot));
    (accounts, bad)
}

fn stored(credential: &Credential) -> Value {
    let stored = Stored {
        kind: "oauth".into(),
        access: credential.access.clone(),
        refresh: credential.refresh.clone(),
        expires: credential.expires,
        account_id: credential.account_id.clone(),
    };
    serde_json::to_value(stored).unwrap_or_default()
}

/// The outcome of [`AccountStore::refresh`].
pub struct Refreshed {
    pub account: Account,
    /// Set when the refreshed credential could not be written.
    pub save_error: Option<std::io::Error>,
}

/// The file under its cross-process lock, as read after the lock was taken.
pub struct Locked<'a> {
    store: &'a AccountStore,
    entries: Entries,
    _lock: FileLock,
}

impl Locked<'_> {
    pub fn get(&self, provider: &str) -> Option<Account> {
        let value = self.entries.get(provider)?;
        parse_entry(provider, value)
    }

    fn write(&mut self) -> std::io::Result<()> {
        self.store.set_memory(parse_entries(&self.entries).0);
        let file = json!({ "version": 1, "accounts": self.entries });
        write_json(&self.store.path, &file)
    }

    pub fn upsert(&mut self, account: &Account) -> std::io::Result<()> {
        self.entries
            .insert(account.provider.clone(), stored(&account.credential));
        self.write()
    }

    pub fn remove(&mut self, provider: &str) -> std::io::Result<bool> {
        let removed = self.entries.remove(provider).is_some();
        self.write()?;
        Ok(removed)
    }
}

impl AccountStore {
    /// Reads the file. Invalid entries are skipped with a warning; an unreadable file starts
    /// empty and every later change fails until the file is fixed.
    pub fn load(path: PathBuf) -> Self {
        let accounts = match read_entries(&path) {
            Ok(entries) => {
                let (accounts, bad) = parse_entries(&entries);
                for provider in bad {
                    pi_ext_sdk::warn!("skipping invalid entry {provider} in {}", path.display());
                }
                accounts
            }
            Err(error) => {
                pi_ext_sdk::warn!("{error}");
                Vec::new()
            }
        };
        Self {
            path,
            accounts: Mutex::new(accounts),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Account>> {
        self.accounts.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn set_memory(&self, accounts: Vec<Account>) {
        *self.lock() = accounts;
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

    /// Takes the cross-process lock and re-reads the file, which also refreshes [`Self::list`].
    pub async fn begin(&self) -> std::io::Result<Locked<'_>> {
        let lock = FileLock::acquire(&self.path).await?;
        let entries = read_entries(&self.path)?;
        self.set_memory(parse_entries(&entries).0);
        Ok(Locked {
            store: self,
            entries,
            _lock: lock,
        })
    }

    pub async fn upsert(&self, account: Account) -> std::io::Result<()> {
        self.begin().await?.upsert(&account)
    }

    pub async fn remove(&self, provider: &str) -> std::io::Result<bool> {
        self.begin().await?.remove(provider)
    }

    /// Refreshes `account` with `refresh` under the file lock. When the file holds a newer or
    /// still-fresh credential (another process refreshed it), that one is returned unrefreshed.
    pub async fn refresh<F, Fut>(&self, account: &Account, refresh: F) -> Result<Refreshed, String>
    where
        F: FnOnce(Account) -> Fut,
        Fut: Future<Output = Result<Credential, String>>,
    {
        let mut locked = self.begin().await.map_err(|e| e.to_string())?;
        let current = locked
            .get(&account.provider)
            .ok_or_else(|| format!("{} is no longer stored", account.provider))?;
        let newer = current.credential.expires > account.credential.expires;
        if newer || !current.needs_refresh(now_ms()) {
            return Ok(Refreshed {
                account: current,
                save_error: None,
            });
        }
        let credential = refresh(current.clone()).await?;
        let account = Account {
            credential,
            ..current
        };
        let save_error = locked.upsert(&account).err();
        Ok(Refreshed {
            account,
            save_error,
        })
    }
}
