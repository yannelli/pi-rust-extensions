use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use pi_ext_sdk::{Host, LlmRequest, NotifyType, Snapshot};
use serde_json::{Value, json};

use crate::agents::completion::Completer;
use crate::config::{
    Config, PASSIVE_ENV, SETTINGS_NAMESPACE, bounded_max_tokens, env_passive, merge, read_json_file,
};
use crate::runtime::{MemoryHost, Runtime};

pub fn state_dir(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join(SETTINGS_NAMESPACE)
}

pub fn global_config_path(agent_dir: &Path) -> PathBuf {
    agent_dir
        .join("ext")
        .join(format!("{SETTINGS_NAMESPACE}.json"))
}

pub fn project_config_path(cwd: &Path) -> PathBuf {
    cwd.join(".pi")
        .join("ext")
        .join(format!("{SETTINGS_NAMESPACE}.json"))
}

pub fn project_trusted(snapshot: &Snapshot) -> bool {
    snapshot
        .extra
        .get("isProjectTrusted")
        .and_then(Value::as_bool)
        == Some(true)
}

/// Defaults, global file, project file (trusted projects only), then the settings namespace.
pub async fn load_config(host: &Host, snapshot: &Snapshot, runtime: &Runtime) -> Config {
    let cwd = if snapshot.cwd.is_empty() {
        host.cwd()
    } else {
        snapshot.cwd_path()
    };
    let global = read_json_file(&global_config_path(&host.agent_dir()));
    let project_path = project_config_path(&cwd);
    let project = if project_trusted(snapshot) {
        read_json_file(&project_path)
    } else {
        if project_path.is_file()
            && !runtime
                .project_untrusted_notified
                .swap(true, Ordering::AcqRel)
            && host.has_ui()
        {
            let message = format!(
                "Observational memory: ignoring {} because this project is not trusted",
                project_path.display()
            );
            let _ = host.notify_warning(message).await;
        }
        Value::Null
    };
    let settings = host
        .settings::<Value>(SETTINGS_NAMESPACE)
        .await
        .ok()
        .flatten()
        .unwrap_or(Value::Null);
    let passive = env_passive(std::env::var(PASSIVE_ENV).ok().as_deref());
    merge(&[global, project, settings], passive)
}

/// The model the memory workers call and the limits that derive from it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MemoryModel {
    pub model_ref: Option<String>,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
    pub reasoning: bool,
}

fn model_limits(model: &Value) -> (Option<u64>, Option<u64>, bool) {
    (
        model.get("contextWindow").and_then(Value::as_u64),
        model.get("maxTokens").and_then(Value::as_u64),
        model
            .get("reasoning")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    )
}

pub async fn resolve_memory_model(
    host: &Host,
    config: &Config,
    snapshot: &Snapshot,
) -> MemoryModel {
    if let Some(configured) = &config.model {
        let args = vec![json!(configured.provider), json!(configured.id)];
        let found = host.pi_call("ctx.modelRegistry", "find", args).await.ok();
        if let Some(model) = found.filter(Value::is_object) {
            let (context_window, max_tokens, reasoning) = model_limits(&model);
            return MemoryModel {
                model_ref: Some(format!("{}/{}", configured.provider, configured.id)),
                context_window,
                max_tokens,
                reasoning,
            };
        }
        if snapshot.has_ui {
            let message = format!(
                "Observational memory: configured model {}/{} not found, using session model",
                configured.provider, configured.id
            );
            let _ = host.notify_warning(message).await;
        }
    }
    let session = snapshot.model.as_ref();
    MemoryModel {
        model_ref: None,
        context_window: session.and_then(|m| m.context_window),
        max_tokens: session.and_then(|m| m.max_tokens),
        reasoning: session.and_then(|m| m.reasoning).unwrap_or(false),
    }
}

/// `llm.complete` on the memory model.
pub struct HostCompleter {
    pub host: Host,
    pub request: LlmRequest,
}

impl HostCompleter {
    pub fn new(host: Host, model: &MemoryModel, config: &Config) -> Self {
        let mut request = LlmRequest::default().max_tokens(bounded_max_tokens(
            model.max_tokens,
            config.agent_max_tokens,
        ));
        request.model = model.model_ref.clone();
        if model.reasoning && config.thinking() != "off" {
            request = request.reasoning(config.thinking());
        }
        Self { host, request }
    }
}

impl Completer for HostCompleter {
    async fn complete(&self, system: &str, prompt: &str) -> Result<String, String> {
        let mut request = self.request.clone().system(system);
        request.prompt = Some(prompt.to_owned());
        self.host
            .complete(request)
            .await
            .map(|response| response.text)
            .map_err(|error| error.message().to_owned())
    }
}

pub struct HostMemory(pub Host);

impl MemoryHost for HostMemory {
    async fn branch(&self) -> Result<Vec<Value>, String> {
        self.0
            .branch(None)
            .await
            .map_err(|e| e.message().to_owned())
    }

    async fn append(&self, custom_type: &str, data: Value) -> Result<(), String> {
        self.0
            .append_entry(custom_type, &data)
            .await
            .map_err(|e| e.message().to_owned())
    }

    async fn notify(&self, message: String, level: &str) {
        let kind = match level {
            "warning" => NotifyType::Warning,
            "error" => NotifyType::Error,
            _ => NotifyType::Info,
        };
        let _ = self.0.notify_as(message, kind).await;
    }

    async fn context_tokens(&self) -> Option<u64> {
        let usage = self.0.context_usage().await.ok().flatten()?;
        usage
            .get("tokens")
            .and_then(Value::as_f64)
            .filter(|t| t.is_finite())
            .map(|t| t as u64)
    }
}
