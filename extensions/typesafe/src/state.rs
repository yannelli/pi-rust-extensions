use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use pi_ext_sdk::paths::project_ext_config;
use pi_ext_sdk::{CustomMessage, Host, NotifyType};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::auth_state::{AuthState, Level, describe};
use crate::backends::{
    BACKEND_ENV, BASE_URL_ENV, Backend, DEFAULT_BACKEND, MODEL_ENV, backend, from_values,
};
use crate::client::Client;
use crate::credentials::{Env, KeyStore, write_private};
use crate::errors::{TsError, TsResult};
use crate::gate::GateState;
use crate::usage::{Caps, Clock, local_day};

const FIXED_CONSENT: &str = "Submitted state and questions will be sent to api.typesafe.ai and may incur charges. Do not include secrets. The extension does not collect files or conversation history. Results are model judgments, not proof or authorization.";

/// `<agentDir>/ext/typesafe.json`. Environment variables win over these keys.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub enabled: Option<bool>,
    pub backend: Option<String>,
    pub model: Option<String>,
    /// The listed input price of `model`, saved by `/typesafe models`.
    pub price_usd_per_mtok: Option<f64>,
    pub max_requests_per_day: Option<u64>,
    pub max_input_tokens_per_day: Option<u64>,
    pub max_usd_per_day: Option<f64>,
    /// The tool-call gate object; `GateConfig::parse` reads it key by key.
    pub gate: Option<Value>,
}

impl Config {
    pub fn path(agent_dir: &Path) -> PathBuf {
        agent_dir.join("ext").join("typesafe.json")
    }

    pub fn load(agent_dir: &Path) -> Config {
        Self::read(&Self::path(agent_dir))
    }

    /// Sets keys of the global file and keeps the others; a null value removes the key.
    pub fn save(agent_dir: &Path, changes: &[(&str, Value)]) -> TsResult<PathBuf> {
        let path = Self::path(agent_dir);
        let refuse = || TsError::configuration(format!("Could not update {}.", path.display()));
        let mut keys: Map<String, Value> = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|_| {
                TsError::configuration(format!(
                    "{} is not a JSON object; fix it first, nothing was saved.",
                    path.display()
                ))
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Map::new(),
            Err(_) => return Err(refuse()),
        };
        for (key, value) in changes {
            match value {
                Value::Null => keys.remove(*key),
                value => keys.insert((*key).to_owned(), value.clone()),
            };
        }
        let text = serde_json::to_string_pretty(&keys).map_err(|_| refuse())?;
        write_private(&path, &format!("{text}\n")).map_err(|_| refuse())?;
        Ok(path)
    }

    /// Sets `gate.enabled` in the global file and keeps the other gate keys.
    pub fn save_gate_enabled(agent_dir: &Path, enabled: bool) -> TsResult<PathBuf> {
        let mut gate = std::fs::read_to_string(Self::path(agent_dir))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|keys| keys.get("gate").and_then(Value::as_object).cloned())
            .unwrap_or_default();
        gate.insert("enabled".to_owned(), Value::Bool(enabled));
        Self::save(agent_dir, &[("gate", Value::Object(gate))])
    }

    pub fn read(path: &Path) -> Config {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        serde_json::from_str(&text).unwrap_or_default()
    }

    fn caps(&self) -> Caps {
        Caps {
            max_requests_per_day: self.max_requests_per_day.filter(|v| *v > 0),
            max_input_tokens_per_day: self.max_input_tokens_per_day.filter(|v| *v > 0),
            max_usd_per_day: self.max_usd_per_day.filter(|v| v.is_finite() && *v > 0.0),
        }
    }
}

/// The project file `.pi/ext/typesafe.json` (the main checkout's copy in a worktree without
/// one) and whether pi trusts the project.
#[derive(Debug, Clone, Default)]
pub struct Project {
    pub config: Config,
    pub trusted: bool,
}

impl Project {
    pub fn load(cwd: &Path, trusted: bool) -> Project {
        let config = Config::read(&project_ext_config(cwd, "typesafe"));
        Project { config, trusted }
    }

    /// `enabled` opens network connections, so it needs trust; caps only lower limits.
    pub fn ignored_enable(&self) -> bool {
        !self.trusted && self.config.enabled.is_some()
    }
}

#[derive(Default)]
struct Session {
    enabled: bool,
    caps: Caps,
    client: Option<Arc<Client>>,
    called_out: Option<String>,
}

/// The model requests send, and a saved price with the model it was listed for.
#[derive(Debug, Clone, Default)]
struct Selection {
    model: Option<String>,
    price: Option<(String, f64)>,
}

/// Process-wide settings plus the per-session gate, client, and callout.
pub struct State {
    pub agent_dir: PathBuf,
    pub dir: PathBuf,
    pub env: Env,
    pub backend: Backend,
    pub backend_error: Option<String>,
    pub clock: Clock,
    selection: RwLock<Selection>,
    session: Mutex<Session>,
    project_notice: AtomicBool,
    pub gate: GateState,
}

impl State {
    /// The backend is fixed for the process: it decides the request host and the tool description.
    pub fn new(env: Env, agent_dir: &Path) -> Arc<State> {
        let config = Config::load(agent_dir);
        let name = env.get(BACKEND_ENV).or(config.backend.clone());
        let saved = config.model.filter(|m| !m.trim().is_empty());
        let price = saved.clone().zip(
            config
                .price_usd_per_mtok
                .filter(|p| p.is_finite() && *p >= 0.0),
        );
        let model = env.get(MODEL_ENV).or(saved);
        let (backend, backend_error) =
            match from_values(name.as_deref(), env.get(BASE_URL_ENV).as_deref()) {
                Ok(backend) => (backend, None),
                Err(error) => (
                    backend(DEFAULT_BACKEND).expect("default backend"),
                    Some(error.message),
                ),
            };
        let state = State {
            agent_dir: agent_dir.to_owned(),
            dir: agent_dir.join("pi-typesafe"),
            env,
            backend,
            backend_error,
            clock: Arc::new(local_day),
            selection: RwLock::new(Selection { model, price }),
            session: Mutex::new(Session::default()),
            project_notice: AtomicBool::new(false),
            gate: GateState::default(),
        };
        state.reset_session(&Project::default());
        Arc::new(state)
    }

    fn selection(&self) -> Selection {
        self.selection.read().expect("selection lock").clone()
    }

    pub fn model(&self) -> Option<String> {
        self.selection().model
    }

    /// Sends `model` from the next request on; `price` is its listed input price.
    pub fn set_model(&self, model: &str, price: Option<f64>) {
        *self.selection.write().expect("selection lock") = Selection {
            model: Some(model.to_owned()),
            price: price.map(|p| (model.to_owned(), p)),
        };
        self.drop_client();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Session> {
        self.session.lock().expect("session lock")
    }

    /// Re-reads the opt-in and caps and drops the client, which resets the session counters.
    /// The environment wins, then a trusted project file, then the global file.
    pub fn reset_session(&self, project: &Project) {
        let global = Config::load(&self.agent_dir);
        let project_enabled = project.config.enabled.filter(|_| project.trusted);
        let configured = project_enabled.or(global.enabled).unwrap_or(false);
        let enabled = self
            .env
            .get("PI_TYPESAFE_ENABLED")
            .map_or(configured, |v| v == "1");
        let caps = global.caps().lowest(project.config.caps());
        self.gate.reset(&global, project);
        *self.lock() = Session {
            enabled,
            caps,
            ..Session::default()
        };
    }

    /// True after the first call: the untrusted-project notice shows once per process.
    pub fn project_notice_shown(&self) -> bool {
        self.project_notice.swap(true, Ordering::Relaxed)
    }

    pub fn enabled(&self) -> bool {
        self.lock().enabled
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.lock().enabled = enabled;
    }

    pub fn client(&self) -> Option<Arc<Client>> {
        self.lock().client.clone()
    }

    pub fn drop_client(&self) {
        self.lock().client = None;
    }

    pub fn get_client(&self) -> TsResult<Arc<Client>> {
        let mut session = self.lock();
        if let Some(client) = &session.client {
            return Ok(client.clone());
        }
        let selection = self.selection();
        let client = Client::new(
            self.backend.clone(),
            None,
            selection.model.as_deref(),
            &self.dir,
            &self.env,
            session.caps,
            self.clock.clone(),
        )?;
        let priced = selection.price.as_ref().map(|(m, p)| (m.as_str(), *p));
        let client = Arc::new(client.with_price(priced));
        session.client = Some(client.clone());
        Ok(client)
    }

    pub fn keys(&self) -> KeyStore {
        KeyStore::new(&self.dir)
    }

    pub fn auth(&self) -> AuthState {
        AuthState::new(&self.dir)
    }

    pub fn describe_auth(&self) -> (Level, String) {
        let situation = self.keys().situation(&self.backend, &self.env);
        describe(&self.backend, &situation, &self.auth().read())
    }

    /// The consent sentence and whether it names the model sent.
    pub fn consent(&self) -> (String, bool) {
        let model = self.model();
        if self.backend.name == DEFAULT_BACKEND && model.is_none() {
            return (FIXED_CONSENT.to_owned(), false);
        }
        let model = Value::String(self.backend.sent_model(model.as_deref()));
        let text = format!(
            "Submitted state and questions will be sent to {} ({}) as model {model} and may incur charges. Do not include secrets. The extension does not collect files or conversation history. Results are model judgments, not proof or authorization.",
            self.backend.host_name(),
            self.backend.label
        );
        (text, true)
    }

    /// One notice per distinct degradation per session.
    pub async fn call_out(&self, host: &Host, key: &str, text: &str) {
        {
            let mut session = self.lock();
            if session.called_out.as_deref() == Some(key) {
                return;
            }
            session.called_out = Some(key.to_owned());
        }
        report(host, text, NotifyType::Warning).await;
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;

/// A notification with UI, else a displayed `typesafe-status` message.
pub async fn report(host: &Host, text: &str, level: NotifyType) {
    let sent = match host.has_ui() {
        true => host.notify_as(text, level).await,
        false => {
            host.send_message(CustomMessage::new("typesafe-status", text).display(true))
                .await
        }
    };
    if let Err(error) = sent {
        pi_ext_sdk::warn!("typesafe: could not report status: {}", error.message());
    }
}
