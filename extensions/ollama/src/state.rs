//! The Ollama host setting, the discovered models, and provider registration.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use pi_ext_sdk::prelude::*;
use serde_json::Map;

use crate::client::{Client, DEFAULT_HOST, normalize_host};
use crate::discovery::{ModelInfo, discover, provider_config};

pub const PROVIDER: &str = "ollama";

pub fn global_config_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join("ollama.json")
}

pub fn project_config_path(cwd: &Path) -> PathBuf {
    cwd.join(".pi").join("ext").join("ollama.json")
}

fn read_host(path: &Path) -> Option<String> {
    let value: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    value
        .get("host")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|h| !h.trim().is_empty())
}

/// A trusted project host, else the global setting, else `OLLAMA_HOST`, else the default.
pub fn resolve_host(
    project: Option<String>,
    global: Option<String>,
    env: Option<String>,
) -> String {
    let raw = project.or(global).or(env.filter(|e| !e.trim().is_empty()));
    raw.map_or_else(|| DEFAULT_HOST.to_string(), |raw| normalize_host(&raw))
}

/// The project host and whether it was ignored for want of trust.
pub fn project_host(cwd: &Path, trusted: bool) -> (Option<String>, bool) {
    match read_host(&project_config_path(cwd)) {
        Some(host) if trusted => (Some(host), false),
        Some(_) => (None, true),
        None => (None, false),
    }
}

pub fn global_host(agent_dir: &Path) -> Option<String> {
    read_host(&global_config_path(agent_dir))
}

pub fn write_global_host(agent_dir: &Path, host: &str) -> std::result::Result<(), String> {
    let path = global_config_path(agent_dir);
    let mut config: Map<String, Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    config.insert("host".into(), json!(host));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(config)).map_err(|e| e.to_string())?;
    std::fs::write(&path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))
}

#[derive(Default)]
struct Inner {
    host: Option<String>,
    models: Vec<ModelInfo>,
    registered: bool,
    warned_untrusted: bool,
}

pub struct Ollama {
    env_host: Option<String>,
    inner: Mutex<Inner>,
}

impl Ollama {
    pub fn new(env_host: Option<String>, initial: Option<(String, Vec<ModelInfo>)>) -> Self {
        let mut inner = Inner::default();
        if let Some((host, models)) = initial {
            inner.registered = !models.is_empty();
            inner.host = Some(host);
            inner.models = models;
        }
        Self {
            env_host,
            inner: Mutex::new(inner),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("ollama lock")
    }

    pub fn models(&self) -> Vec<ModelInfo> {
        self.lock().models.clone()
    }

    /// The host for this session, from the config files and `OLLAMA_HOST`.
    pub async fn host(&self, host: &Host) -> String {
        let trusted = match host
            .snapshot()
            .and_then(|s| s.extra.get("isProjectTrusted").and_then(Value::as_bool))
        {
            Some(trusted) => trusted,
            None => host
                .pi_call("ctx", "isProjectTrusted", vec![])
                .await
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        };
        let (project, ignored) = project_host(&host.cwd(), trusted);
        let warn = ignored && !std::mem::replace(&mut self.lock().warned_untrusted, true);
        if warn {
            let text = "ollama: ignored the host in .pi/ext/ollama.json because the project is not trusted.";
            let _ = host.notify_warning(text).await;
        }
        let url = resolve_host(
            project,
            global_host(&host.agent_dir()),
            self.env_host.clone(),
        );
        self.lock().host = Some(url.clone());
        url
    }

    pub async fn client(&self, host: &Host) -> Client {
        Client::new(&self.host(host).await)
    }

    /// Discovers the models and registers provider `ollama`; returns the model count.
    pub async fn refresh(&self, host: &Host) -> std::result::Result<usize, String> {
        let client = self.client(host).await;
        let discovered = discover(&client).await;
        let models = match discovered {
            Ok(models) => models,
            Err(error) => {
                self.unregister(host).await;
                self.lock().models.clear();
                return Err(error);
            }
        };
        let chat = models.iter().filter(|m| m.can_chat()).count();
        self.lock().models = models.clone();
        if chat == 0 {
            self.unregister(host).await;
            return Ok(0);
        }
        let config = provider_config(client.base(), &models);
        host.register_provider(PROVIDER, config)
            .await
            .map_err(|e| e.message().to_string())?;
        self.lock().registered = true;
        Ok(chat)
    }

    async fn unregister(&self, host: &Host) {
        if std::mem::take(&mut self.lock().registered) {
            let _ = host.unregister_provider(PROVIDER).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_precedence() {
        let s = |v: &str| Some(v.to_string());
        assert_eq!(resolve_host(None, None, None), DEFAULT_HOST);
        assert_eq!(resolve_host(None, None, s("box:1234")), "http://box:1234");
        assert_eq!(resolve_host(None, s("http://g:1"), s("box")), "http://g:1");
        assert_eq!(
            resolve_host(s("p"), s("http://g:1"), None),
            "http://p:11434"
        );
        assert_eq!(resolve_host(None, None, s(" ")), DEFAULT_HOST);
    }

    #[test]
    fn project_host_needs_trust_and_global_host_round_trips() {
        let dir = std::env::temp_dir().join(format!("pi-ollama-cfg-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".pi/ext")).unwrap();
        std::fs::write(project_config_path(&dir), r#"{"host": "evil:1"}"#).unwrap();
        assert_eq!(project_host(&dir, false), (None, true));
        assert_eq!(project_host(&dir, true), (Some("evil:1".into()), false));
        write_global_host(&dir, "http://127.0.0.1:9").unwrap();
        assert_eq!(global_host(&dir).as_deref(), Some("http://127.0.0.1:9"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
