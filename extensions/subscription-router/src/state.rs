//! Shared extension state and the pi-backed `RouteEnv` and `Tokens`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use pi_ext_sdk::Host;
use serde_json::{Value, json};

use crate::accounts::AccountStore;
use crate::config::{self, Config, ModelRef, ROUTER_PROVIDER};
use crate::failover::Failover;
use crate::family::{Family, parse_provider};
use crate::router::{RouteDecision, RouteEnv, Router};
use crate::slots::SlotInfo;
use crate::usage::{
    BoxFuture, GetOptions, Tokens, Urls, UsageDecision, UsageService, decision_for,
};

const RECENT_DECISIONS: usize = 5;
pub const DECISION_ENTRY: &str = "router:decision";

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

pub struct State {
    pub agent_dir: PathBuf,
    pub config: Mutex<Config>,
    pub config_error: Mutex<Option<String>>,
    pub router: Router,
    pub accounts: AccountStore,
    pub http: reqwest::Client,
    pub recent: Mutex<VecDeque<RouteDecision>>,
    pub persisted: Mutex<HashMap<String, String>>,
    pub failover: Mutex<Failover>,
    /// Profiles registered as virtual models with pi.
    pub registered: Mutex<HashSet<String>>,
    /// Chain models recovered at a settle boundary during the current prompt.
    pub recovered: Mutex<HashSet<String>>,
}

impl State {
    pub fn new(agent_dir: &Path) -> Self {
        Self::with_urls(agent_dir, Urls::from_env())
    }

    pub fn with_urls(agent_dir: &Path, urls: Urls) -> Self {
        let data = agent_dir.join("ext/subscription-router");
        let (config, error) = match config::load(&config::config_path(agent_dir)) {
            Ok(config) => (config, None),
            Err(error) => (Config::default(), Some(error)),
        };
        let usage = UsageService::new(urls, Some(data.join("usage.json")));
        Self {
            agent_dir: agent_dir.to_owned(),
            registered: Mutex::new(config.profiles.iter().map(|p| p.id.clone()).collect()),
            config: Mutex::new(config),
            config_error: Mutex::new(error),
            router: Router::new(Arc::new(usage)),
            accounts: AccountStore::load(data.join("accounts.json")),
            http: reqwest::Client::new(),
            recent: Mutex::new(VecDeque::new()),
            persisted: Mutex::new(HashMap::new()),
            failover: Mutex::new(Failover::default()),
            recovered: Mutex::new(HashSet::new()),
        }
    }

    pub fn usage(&self) -> &UsageService {
        &self.router.usage
    }

    pub fn config(&self) -> Config {
        lock(&self.config).clone()
    }

    /// Keeps the last five decisions; returns true when this one goes to the session.
    pub fn record(&self, decision: &RouteDecision) -> bool {
        let mut recent = lock(&self.recent);
        recent.push_back(decision.clone());
        while recent.len() > RECENT_DECISIONS {
            recent.pop_front();
        }
        let mut persisted = lock(&self.persisted);
        if persisted.get(&decision.profile) == Some(&decision.chosen) {
            return false;
        }
        persisted.insert(decision.profile.clone(), decision.chosen.clone());
        true
    }

    /// Slot 1 plus the extension slots of `family`, with cached usage.
    pub fn slots(&self, family: Family) -> Vec<SlotInfo> {
        let mut slots = vec![1];
        slots.extend(
            self.accounts
                .list()
                .iter()
                .filter(|a| a.family == family)
                .map(|a| a.slot),
        );
        slots
            .into_iter()
            .map(|slot| SlotInfo {
                family,
                slot,
                snapshot: self.usage().peek(&family.provider(slot)),
            })
            .collect()
    }

    /// Providers with a usage collector that chains, slots, or the current model use.
    pub fn session_providers(&self, current: Option<&str>) -> Vec<String> {
        let mut providers: Vec<String> = Vec::new();
        let mut push = |p: String| {
            if parse_provider(&p).is_some() && !providers.contains(&p) {
                providers.push(p);
            }
        };
        for profile in &lock(&self.config).profiles {
            profile
                .chain
                .iter()
                .for_each(|e| push(e.model.provider.clone()));
        }
        self.accounts
            .list()
            .into_iter()
            .for_each(|a| push(a.provider));
        if let Some(current) = current.filter(|p| *p != ROUTER_PROVIDER) {
            push(current.to_owned());
        }
        providers
    }
}

/// Bearer tokens: the router's own token of an older slot, else pi's auth for the provider.
pub struct HostTokens<'a> {
    pub host: &'a Host,
    pub state: &'a State,
}

impl Tokens for HostTokens<'_> {
    fn token<'a>(&'a self, provider: &'a str) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            let own = self.state.accounts.get(provider).and_then(|a| a.credential);
            if let Some(credential) = own {
                return Some(credential.access);
            }
            self.host
                .provider_auth(provider)
                .await
                .ok()
                .flatten()?
                .api_key
        })
    }
}

/// `RouteEnv` over pi's model registry; availability is read once per route.
pub struct HostEnv {
    host: Host,
    state: Arc<State>,
    available: HashSet<String>,
}

impl HostEnv {
    pub async fn build(host: &Host, state: &Arc<State>, models: &[ModelRef]) -> Self {
        let mut available = HashSet::new();
        for model in models {
            if model_available(host, model).await {
                available.insert(model.key());
            }
        }
        Self {
            host: host.clone(),
            state: state.clone(),
            available,
        }
    }
}

/// `find` and `hasConfiguredAuth` of `ctx.modelRegistry`; a failed call counts as available.
pub async fn model_available(host: &Host, model: &ModelRef) -> bool {
    let args = vec![json!(model.provider), json!(model.id)];
    let found = match host.pi_call("ctx.modelRegistry", "find", args).await {
        Ok(found) => found,
        Err(_) => return true,
    };
    if found.is_null() {
        return false;
    }
    let args = vec![json!({ "provider": model.provider, "id": model.id })];
    let auth = host
        .pi_call("ctx.modelRegistry", "hasConfiguredAuth", args)
        .await;
    auth.map_or(true, |v| v.as_bool().unwrap_or(false))
}

impl RouteEnv for HostEnv {
    fn available(&self, model: &ModelRef) -> bool {
        self.available.contains(&model.key())
    }

    fn refresh<'a>(
        &'a self,
        model: &'a ModelRef,
        not_before: i64,
    ) -> BoxFuture<'a, Option<UsageDecision>> {
        Box::pin(async move {
            let tokens = HostTokens {
                host: &self.host,
                state: &self.state,
            };
            let options = GetOptions {
                fresh: true,
                not_before: Some(not_before),
                ..Default::default()
            };
            let snapshot = self
                .state
                .usage()
                .get(&model.provider, &tokens, options)
                .await;
            let read = snapshot.status == crate::usage::Status::Available
                && snapshot.retrieved_at >= not_before;
            read.then(|| decision_for(model, Some(&snapshot), crate::fsutil::now_ms()))
        })
    }
}

/// Warms cold caches in the background; the hot path never waits for it.
pub fn warm(host: &Host, state: &Arc<State>, providers: Vec<String>) {
    let (host, state) = (host.clone(), state.clone());
    tokio::spawn(async move {
        for provider in providers {
            let fresh = state
                .usage()
                .peek(&provider)
                .is_some_and(|s| s.status == crate::usage::Status::Available);
            if !fresh {
                let tokens = HostTokens {
                    host: &host,
                    state: &state,
                };
                state
                    .usage()
                    .get(&provider, &tokens, GetOptions::default())
                    .await;
            }
        }
    });
}

pub fn current_model(host: &Host) -> Option<ModelRef> {
    let model = host.snapshot()?.model.clone()?;
    Some(ModelRef::new(model.provider, model.id))
}

pub fn value_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}
