use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use pi_ext_sdk::CancellationToken;
use serde_json::{Value, json};

use crate::auth_state::AuthState;
use crate::backends::Backend;
use crate::caps::{cap_label, caps_description, number};
use crate::credentials::{Env, KeySituation, KeyStore};
use crate::errors::{Code, TsError, TsResult};
use crate::result::valid_result;
use crate::usage::{Blocked, Caps, Clock, DEFAULT_USD_PER_MTOK, Ledger, Today};
use crate::validate::{MAX_INPUT_BYTES, check_bytes, prepare};

pub const DEFAULT_MAX_REQUESTS: u64 = 20;
pub const TIMEOUT: Duration = Duration::from_secs(15);

/// Attempts, outcomes and tokens of one client instance (one session).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Session {
    pub requests_started: u64,
    pub requests_succeeded: u64,
    pub requests_failed: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub usd: f64,
}

pub struct Spend {
    pub session: Session,
    pub today: Today,
    pub blocked: Option<Blocked>,
}

pub struct Client {
    http: reqwest::Client,
    pub backend: Backend,
    key: String,
    pub model: String,
    caps: Caps,
    ledger: Ledger,
    auth: AuthState,
    session: Mutex<Session>,
    verified: AtomicBool,
    last_failure: Mutex<Option<String>>,
}

impl Client {
    /// `key` overrides the configured key (login verification uses it).
    pub fn new(
        backend: Backend,
        key: Option<String>,
        model: Option<&str>,
        dir: &Path,
        env: &Env,
        caps: Caps,
        clock: Clock,
    ) -> TsResult<Client> {
        let key = match key {
            Some(key) => key,
            None => match KeyStore::new(dir).situation(&backend, env) {
                KeySituation::Unusable { reason } => return Err(TsError::configuration(reason)),
                KeySituation::Missing => {
                    return Err(TsError::configuration(format!(
                        "No API key. Run /typesafe login {} in Pi, or set {} in the environment.",
                        backend.name, backend.key_env
                    )));
                }
                situation => situation.key().unwrap_or_default().to_owned(),
            },
        };
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(concat!("pi-ext-typesafe/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| TsError::configuration("Could not build the HTTP client."))?;
        let model = backend.sent_model(model);
        let ledger = Ledger::open(dir.join("usage.json"), DEFAULT_USD_PER_MTOK, clock);
        Ok(Client {
            http,
            key,
            model,
            caps: caps.lowest(Caps::from_env(env)),
            ledger,
            auth: AuthState::new(dir),
            backend,
            session: Mutex::new(Session::default()),
            verified: AtomicBool::new(false),
            last_failure: Mutex::new(None),
        })
    }

    pub fn spend(&self) -> Spend {
        let session = *self.session.lock().expect("session lock");
        let blocked = match session.requests_started >= DEFAULT_MAX_REQUESTS {
            true => None,
            false => self.ledger.blocked(&self.caps),
        };
        Spend {
            session,
            today: self.ledger.today(),
            blocked,
        }
    }

    fn record_verified(&self) {
        if !self.verified.swap(true, Ordering::Relaxed) {
            self.auth.record_verified();
        }
    }

    async fn send(
        &self,
        request: reqwest::RequestBuilder,
        cancel: &CancellationToken,
    ) -> TsResult<Value> {
        let request = request
            .bearer_auth(&self.key)
            .header("Accept", "application/json");
        let exchange = async {
            let response = request.send().await.map_err(|e| match e.is_timeout() {
                true => TsError::timeout(),
                false => TsError::connection(),
            })?;
            let status = response.status().as_u16();
            if !response.status().is_success() {
                let retry = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                return Err(TsError::http(status, retry.as_deref(), Some(&self.backend)));
            }
            let body = response.bytes().await.map_err(|e| match e.is_timeout() {
                true => TsError::timeout(),
                false => TsError::connection(),
            })?;
            serde_json::from_slice(&body).map_err(|_| TsError::response())
        };
        tokio::select! {
            result = exchange => result,
            () = cancel.cancelled() => Err(TsError::aborted()),
        }
    }

    pub async fn list_models(&self, cancel: &CancellationToken) -> TsResult<Vec<String>> {
        let wire = self
            .send(self.http.get(self.backend.models_url()), cancel)
            .await?;
        let list = wire
            .get(self.backend.models_field)
            .and_then(Value::as_array)
            .ok_or_else(|| {
                TsError::new(
                    Code::Response,
                    "TypeSafe returned an unexpected model list.",
                )
            })?;
        if self.backend.models_verify_key {
            self.record_verified();
        }
        let id = |entry: &Value| {
            let named = entry
                .get(self.backend.models_id_field)
                .or_else(|| entry.get("name"));
            named
                .and_then(Value::as_str)
                .filter(|n| (1..=100).contains(&n.len()))
                .map(str::to_owned)
        };
        Ok(list.iter().filter_map(id).collect())
    }

    /// Checks the session and daily caps and counts the attempt under one lock.
    fn reserve(&self) -> TsResult<()> {
        let mut session = self.session.lock().expect("session lock");
        if session.requests_started >= DEFAULT_MAX_REQUESTS {
            return Err(TsError::new(
                Code::Budget,
                format!(
                    "TypeSafe request limit reached ({DEFAULT_MAX_REQUESTS} attempts per client instance). {}",
                    caps_description(&self.caps)
                ),
            ));
        }
        if let Some(reached) = self.ledger.blocked(&self.caps) {
            return Err(TsError::new(
                Code::Budget,
                format!(
                    "TypeSafe {} reached ({} of {} on {}); no request was submitted. Requests resume after the local day rolls over, or raise the cap deliberately.",
                    cap_label(reached.cap),
                    number(reached.used),
                    number(reached.limit),
                    reached.day
                ),
            ));
        }
        session.requests_started += 1;
        self.ledger.record_start();
        Ok(())
    }

    /// One judge request: validated, capped, counted, and never retried.
    pub async fn evaluate(&self, input: Value, cancel: &CancellationToken) -> TsResult<Value> {
        let mut request = prepare(input, Some(&self.backend.limits))?;
        let sent_model = match request.get("model").and_then(Value::as_str) {
            Some(model) => self.backend.sent_model(Some(model)),
            None => self.model.clone(),
        };
        request["model"] = json!(sent_model);
        check_bytes(&request.to_string(), MAX_INPUT_BYTES)?;
        if cancel.is_cancelled() {
            return Err(TsError::new(
                Code::Aborted,
                "TypeSafe request cancelled before submission.",
            ));
        }
        self.reserve()?;
        let start = Instant::now();
        let post = self.http.post(self.backend.systemone_url()).json(&request);
        let outcome = self.send(post, cancel).await.and_then(|result| {
            match valid_result(&result, &request["questions"]) {
                true => Ok(result),
                false => Err(TsError::new(
                    Code::Response,
                    "TypeSafe returned an unexpected answer or usage format.",
                )),
            }
        });
        match outcome {
            Ok(mut result) => {
                let tokens = |field: &str| result["usage"][field].as_u64().unwrap_or(0);
                let (input, output) = (tokens("input_tokens"), tokens("output_tokens"));
                let rate = self
                    .backend
                    .price(&sent_model)
                    .unwrap_or(DEFAULT_USD_PER_MTOK);
                let cost = input as f64 * rate / 1e6;
                {
                    let mut session = self.session.lock().expect("session lock");
                    session.requests_succeeded += 1;
                    session.input_tokens += input;
                    session.output_tokens += output;
                    session.usd += cost;
                }
                self.ledger.record_success(input, output, cost);
                self.record_verified();
                result["elapsedMs"] = json!(start.elapsed().as_millis() as u64);
                Ok(result)
            }
            Err(error) => {
                self.session.lock().expect("session lock").requests_failed += 1;
                self.ledger.record_failure();
                let fingerprint = format!(
                    "{}:{:?}:{}",
                    error.code.as_str(),
                    error.status,
                    error.message
                );
                let mut last = self.last_failure.lock().expect("failure lock");
                if last.as_deref() != Some(&fingerprint) {
                    *last = Some(fingerprint);
                    self.auth.record_failure(&error);
                }
                Err(error)
            }
        }
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
