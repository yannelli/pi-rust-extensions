//! `<agentDir>/ext/subscription-router.json`: router profiles (upstream schema) and `autoSwitchOn429`.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub profiles: Vec<Profile>,
    pub auto_switch_on_429: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            profiles: Vec::new(),
            auto_switch_on_429: true,
        }
    }
}

impl Config {
    pub fn profile(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
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
    let model = ModelRef::parse(model).ok_or_else(bad_model)?;
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
        None if root.contains_key("autoSwitchOn429") => Vec::new(),
        Some(Value::Object(profiles)) => profiles
            .iter()
            .map(|(id, value)| parse_profile(id, value))
            .collect::<Result<_, _>>()?,
        _ => return Err("profiles: expected an object of id -> profile".into()),
    };
    Ok(Config {
        profiles,
        auto_switch_on_429,
    })
}

mod file;

pub use file::{config_json, config_path, load, profile_json, save};
