use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use pi_ext_sdk::{CustomMessage, Host, NotifyType};
use serde::Deserialize;

use crate::auth_state::{AuthState, Level, describe};
use crate::backends::{
    BACKEND_ENV, BASE_URL_ENV, Backend, DEFAULT_BACKEND, MODEL_ENV, backend, from_values,
};
use crate::client::Client;
use crate::credentials::{Env, KeyStore};
use crate::errors::TsResult;
use crate::usage::{Caps, Clock, local_day};

const FIXED_CONSENT: &str = "Submitted state and questions will be sent to api.typesafe.ai and may incur charges. Do not include secrets. The extension does not collect files or conversation history. Results are model judgments, not proof or authorization.";

/// `<agentDir>/ext/typesafe.json`. Environment variables win over these keys.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub enabled: Option<bool>,
    pub backend: Option<String>,
    pub model: Option<String>,
    pub max_requests_per_day: Option<u64>,
    pub max_input_tokens_per_day: Option<u64>,
    pub max_usd_per_day: Option<f64>,
}

impl Config {
    pub fn load(agent_dir: &Path) -> Config {
        Self::read(&agent_dir.join("ext").join("typesafe.json"))
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

/// The project file `<cwd>/.pi/ext/typesafe.json` and whether pi trusts the project.
#[derive(Debug, Clone, Default)]
pub struct Project {
    pub config: Config,
    pub trusted: bool,
}

impl Project {
    pub fn load(cwd: &Path, trusted: bool) -> Project {
        let config = Config::read(&cwd.join(".pi").join("ext").join("typesafe.json"));
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

/// Process-wide settings plus the per-session gate, client, and callout.
pub struct State {
    pub agent_dir: PathBuf,
    pub dir: PathBuf,
    pub env: Env,
    pub backend: Backend,
    pub model: Option<String>,
    pub backend_error: Option<String>,
    pub clock: Clock,
    session: Mutex<Session>,
    project_notice: AtomicBool,
}

impl State {
    /// The backend and model are fixed for the process: they decide where requests go.
    pub fn new(env: Env, agent_dir: &Path) -> Arc<State> {
        let config = Config::load(agent_dir);
        let name = env.get(BACKEND_ENV).or(config.backend.clone());
        let model = env
            .get(MODEL_ENV)
            .or(config.model.clone())
            .filter(|m| !m.trim().is_empty());
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
            model,
            backend_error,
            clock: Arc::new(local_day),
            session: Mutex::new(Session::default()),
            project_notice: AtomicBool::new(false),
        };
        state.reset_session(&Project::default());
        Arc::new(state)
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
        let client = Client::new(
            self.backend.clone(),
            None,
            self.model.as_deref(),
            &self.dir,
            &self.env,
            session.caps,
            self.clock.clone(),
        )?;
        let client = Arc::new(client);
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
        if self.backend.name == DEFAULT_BACKEND && self.model.is_none() {
            return (FIXED_CONSENT.to_owned(), false);
        }
        let model = serde_json::Value::String(self.backend.sent_model(self.model.as_deref()));
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
