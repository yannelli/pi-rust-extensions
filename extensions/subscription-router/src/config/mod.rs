//! `<agentDir>/ext/subscription-router.json`: router profiles (upstream schema), `autoSwitchOn429`,
//! and the account policies of each subscription provider.

use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::family::{Family, parse_provider};

pub const ROUTER_PROVIDER: &str = "router";
pub const CONFIG_ENV: &str = "PI_ROUTER_CONFIG";
pub const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

static PROFILE_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9._-]*$").expect("valid regex"));

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider: String,
    pub id: String,
}

impl ModelRef {
    pub fn new(provider: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            id: id.into(),
        }
    }

    /// Splits `provider/id`; the id may contain `/`.
    pub fn parse(text: &str) -> Option<Self> {
        let (provider, id) = text.split_once('/')?;
        (!provider.is_empty() && !id.is_empty()).then(|| Self::new(provider, id))
    }

    pub fn key(&self) -> String {
        format!("{}/{}", self.provider, self.id)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChainEntry {
    pub model: ModelRef,
    pub thinking: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Strategy {
    #[default]
    Ordered,
    Balanced,
}

impl Strategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ordered => "ordered",
            Self::Balanced => "balanced",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    Fail,
    Wait,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Exhaustion {
    pub policy: Policy,
    pub max_wait_minutes: Option<f64>,
}

impl Exhaustion {
    pub fn max_wait_ms(&self) -> Option<i64> {
        self.max_wait_minutes.map(|m| (m * 60_000.0) as i64)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub strategy: Option<Strategy>,
    pub chain: Vec<ChainEntry>,
    pub exhaustion: Option<Exhaustion>,
}

impl Profile {
    pub fn strategy(&self) -> Strategy {
        self.strategy.unwrap_or_default()
    }

    pub fn entry(&self, model: &ModelRef) -> Option<&ChainEntry> {
        self.chain.iter().find(|entry| &entry.model == model)
    }
}

/// How the router picks among the accounts of one subscription provider.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AccountStrategy {
    /// Always the chosen `account`.
    Manual,
    /// Stay on an account until it is full, then move to the next one.
    Fill,
    /// Like `fill`, with an account counting as full at `switchAt` percent.
    Threshold,
    /// The account with the most quota left for the time left, with some stickiness.
    #[default]
    Balance,
}

impl AccountStrategy {
    pub const ALL: [AccountStrategy; 4] =
        [Self::Manual, Self::Fill, Self::Threshold, Self::Balance];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Fill => "fill",
            Self::Threshold => "threshold",
            Self::Balance => "balance",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == text)
    }
}

/// When the router spends a banked usage reset of an account (Codex only).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResetUse {
    #[default]
    Off,
    /// As soon as the account in use is full.
    WhenFull,
    /// Only when every account of the provider is full, before falling back.
    WhenAllFull,
}

impl ResetUse {
    pub const ALL: [ResetUse; 3] = [Self::Off, Self::WhenFull, Self::WhenAllFull];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::WhenFull => "when-full",
            Self::WhenAllFull => "when-all-full",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == text)
    }
}

pub const DEFAULT_RESET_MIN_WAIT_MINUTES: f64 = 60.0;

/// The account policy of one subscription provider (`accounts.<provider>`).
#[derive(Clone, Debug, PartialEq)]
pub struct AccountPolicy {
    pub strategy: AccountStrategy,
    /// The account of `manual`; 1 is pi's own login.
    pub account: u32,
    /// Percent used at which `threshold` treats an account as full.
    pub switch_at: f64,
    /// Models of other providers to use when every account is full, in order.
    pub fallback: Vec<ChainEntry>,
    /// Go back to this provider when one of its accounts has room again.
    pub return_when_available: bool,
    pub use_resets: ResetUse,
    /// A full window that resets sooner than this is waited out instead of reset.
    pub reset_min_wait_minutes: f64,
}

impl Default for AccountPolicy {
    fn default() -> Self {
        Self {
            strategy: AccountStrategy::default(),
            account: 1,
            switch_at: 90.0,
            fallback: Vec::new(),
            return_when_available: true,
            use_resets: ResetUse::Off,
            reset_min_wait_minutes: DEFAULT_RESET_MIN_WAIT_MINUTES,
        }
    }
}

impl AccountPolicy {
    /// Percent used at which an account counts as full.
    pub fn full_at(&self) -> f64 {
        match self.strategy {
            AccountStrategy::Threshold => self.switch_at,
            _ => 100.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub profiles: Vec<Profile>,
    pub auto_switch_on_429: bool,
    /// Account policies by subscription provider; a missing one is the default.
    pub accounts: BTreeMap<&'static str, AccountPolicy>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            profiles: Vec::new(),
            auto_switch_on_429: true,
            accounts: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn profile(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn policy(&self, family: Family) -> AccountPolicy {
        self.accounts
            .get(family.base())
            .cloned()
            .unwrap_or_default()
    }
}

fn object<'a>(value: &'a Value, where_: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{where_}: expected an object"))
}

fn parse_entry(raw: &Value, where_: &str) -> Result<ChainEntry, String> {
    let raw = object(raw, where_)?;
    let bad_model = || format!("{where_}.model: expected \"provider/model-id\"");
    let model = raw
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(bad_model)?;
    let mut model = ModelRef::parse(model).ok_or_else(bad_model)?;
    // `{ model: "anthropic/x", account: 2 }` is model x on account 2, kept as `anthropic-2/x`.
    match raw.get("account") {
        None => {}
        Some(value) => {
            let account = value.as_u64().filter(|n| (1..=99).contains(n));
            let account = account
                .ok_or_else(|| format!("{where_}.account: expected a number from 1 to 99"))?;
            let family = parse_provider(&model.provider).filter(|(_, slot)| *slot == 1);
            let Some((family, _)) = family else {
                return Err(format!(
                    "{where_}.account: only anthropic and openai-codex models have accounts"
                ));
            };
            model.provider = family.provider(account as u32);
        }
    }
    if model.provider == ROUTER_PROVIDER {
        return Err(format!(
            "{where_}.model: a chain cannot name another router model"
        ));
    }
    let thinking = match raw.get("thinking") {
        None => None,
        Some(Value::String(level)) if THINKING_LEVELS.contains(&level.as_str()) => {
            Some(level.clone())
        }
        Some(_) => {
            let levels = THINKING_LEVELS.join(", ");
            return Err(format!("{where_}.thinking: expected one of {levels}"));
        }
    };
    Ok(ChainEntry { model, thinking })
}

fn parse_exhaustion(raw: &Map<String, Value>, where_: &str) -> Result<Option<Exhaustion>, String> {
    let (policy, wait) = (raw.get("exhaustionPolicy"), raw.get("maxWaitMinutes"));
    if policy.is_none() && wait.is_none() {
        return Ok(None);
    }
    let policy = match policy.map(|p| p.as_str()) {
        None | Some(Some("wait")) => Policy::Wait,
        Some(Some("fail")) => Policy::Fail,
        Some(_) => {
            return Err(format!(
                "{where_}.exhaustionPolicy: expected one of fail, wait"
            ));
        }
    };
    let max_wait_minutes = match wait {
        None => None,
        Some(value) => match value.as_f64() {
            Some(minutes) if minutes > 0.0 => Some(minutes),
            _ => {
                return Err(format!(
                    "{where_}.maxWaitMinutes: expected a positive number"
                ));
            }
        },
    };
    Ok(Some(Exhaustion {
        policy,
        max_wait_minutes,
    }))
}

fn parse_profile(id: &str, value: &Value) -> Result<Profile, String> {
    let where_ = format!("profiles.{id}");
    if !PROFILE_ID.is_match(id) {
        return Err(format!("{where_}: id must match {}", PROFILE_ID.as_str()));
    }
    let raw = object(value, &where_)?;
    let strategy = match raw.get("strategy").map(Value::as_str) {
        None => None,
        Some(Some("ordered")) => Some(Strategy::Ordered),
        Some(Some("balanced")) => Some(Strategy::Balanced),
        Some(_) => return Err(format!("{where_}.strategy: expected ordered or balanced")),
    };
    let name = match raw.get("name") {
        None => format!("Router {id}"),
        Some(Value::String(name)) => name.clone(),
        Some(_) => return Err(format!("{where_}.name: expected a string")),
    };
    let chain = raw
        .get("chain")
        .and_then(Value::as_array)
        .filter(|c| !c.is_empty());
    let chain = chain.ok_or_else(|| format!("{where_}.chain: expected a non-empty array"))?;
    let chain = chain
        .iter()
        .enumerate()
        .map(|(i, entry)| parse_entry(entry, &format!("{where_}.chain[{i}]")))
        .collect::<Result<Vec<_>, _>>()?;
    let mut seen = HashSet::new();
    if let Some(dup) = chain.iter().find(|entry| !seen.insert(entry.model.key())) {
        return Err(format!(
            "{where_}.chain: {} is listed twice",
            dup.model.key()
        ));
    }
    let exhaustion = parse_exhaustion(raw, &where_)?;
    let id = id.to_owned();
    Ok(Profile {
        id,
        name,
        strategy,
        chain,
        exhaustion,
    })
}

/// Validates a config document; any error rejects the whole file.
pub fn parse_config(raw: &Value) -> Result<Config, String> {
    let root = raw.as_object().ok_or("expected a JSON object")?;
    let auto_switch_on_429 = match root.get("autoSwitchOn429") {
        None => true,
        Some(Value::Bool(value)) => *value,
        Some(_) => return Err("autoSwitchOn429: expected a boolean".into()),
    };
    let profiles = match root.get("profiles") {
        None if root.contains_key("autoSwitchOn429") || root.contains_key("accounts") => Vec::new(),
        Some(Value::Object(profiles)) => profiles
            .iter()
            .map(|(id, value)| parse_profile(id, value))
            .collect::<Result<_, _>>()?,
        _ => return Err("profiles: expected an object of id -> profile".into()),
    };
    let accounts = match root.get("accounts") {
        None => BTreeMap::new(),
        Some(Value::Object(accounts)) => accounts
            .iter()
            .map(|(provider, value)| parse_policy(provider, value))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("accounts: expected an object of provider -> policy".into()),
    };
    Ok(Config {
        profiles,
        auto_switch_on_429,
        accounts,
    })
}

fn parse_policy(provider: &str, value: &Value) -> Result<(&'static str, AccountPolicy), String> {
    let where_ = format!("accounts.{provider}");
    let family = Family::ALL.into_iter().find(|f| f.base() == provider);
    let family = family.ok_or_else(|| format!("{where_}: expected anthropic or openai-codex"))?;
    let raw = object(value, &where_)?;
    let mut policy = AccountPolicy::default();
    if let Some(value) = raw.get("strategy") {
        let strategy = value.as_str().and_then(AccountStrategy::parse);
        policy.strategy = strategy.ok_or_else(|| {
            format!("{where_}.strategy: expected manual, fill, threshold, or balance")
        })?;
    }
    if let Some(value) = raw.get("account") {
        let account = value.as_u64().filter(|n| (1..=99).contains(n));
        policy.account = account
            .ok_or_else(|| format!("{where_}.account: expected a number from 1 to 99"))?
            as u32;
    }
    if let Some(value) = raw.get("switchAt") {
        let percent = value.as_f64().filter(|p| *p > 0.0 && *p <= 100.0);
        policy.switch_at = percent
            .ok_or_else(|| format!("{where_}.switchAt: expected a percent above 0, at most 100"))?;
    }
    if let Some(value) = raw.get("fallback") {
        let entries = value
            .as_array()
            .ok_or_else(|| format!("{where_}.fallback: expected an array"))?;
        policy.fallback = entries
            .iter()
            .enumerate()
            .map(|(i, entry)| parse_entry(entry, &format!("{where_}.fallback[{i}]")))
            .collect::<Result<_, _>>()?;
        if let Some(own) = policy
            .fallback
            .iter()
            .find(|e| parse_provider(&e.model.provider).is_some_and(|(f, _)| f == family))
        {
            return Err(format!(
                "{where_}.fallback: {} is a model of {provider} itself",
                own.model.key()
            ));
        }
    }
    if let Some(value) = raw.get("returnWhenAvailable") {
        policy.return_when_available = value
            .as_bool()
            .ok_or_else(|| format!("{where_}.returnWhenAvailable: expected a boolean"))?;
    }
    if let Some(value) = raw.get("useResets") {
        let reset = value.as_str().and_then(ResetUse::parse);
        policy.use_resets = reset.ok_or_else(|| {
            format!("{where_}.useResets: expected off, when-full, or when-all-full")
        })?;
    }
    if let Some(value) = raw.get("resetMinWaitMinutes") {
        let minutes = value.as_f64().filter(|m| *m >= 0.0);
        policy.reset_min_wait_minutes = minutes
            .ok_or_else(|| format!("{where_}.resetMinWaitMinutes: expected a number of minutes"))?;
    }
    Ok((family.base(), policy))
}

mod file;

pub use file::{config_json, config_path, entry_json, load, policy_json, profile_json, save};
