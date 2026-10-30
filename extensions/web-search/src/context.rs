//! Reads from pi: project trust, full model objects, model credentials, and the merged config.

use std::sync::atomic::{AtomicBool, Ordering};

use pi_ext_sdk::{Host, Snapshot};
use serde_json::{Value, json};

use crate::config::{self, Loaded};
use crate::model::{Auth, Kind, SearchModel, env_api_key};

static IGNORED_NOTICE: AtomicBool = AtomicBool::new(false);

/// `ctx.isProjectTrusted` from the snapshot, else through `pi.call`; `false` when unknown.
pub async fn project_trusted(host: &Host, snapshot: Option<&Snapshot>) -> bool {
    if let Some(trusted) = snapshot
        .and_then(|s| s.extra.get("isProjectTrusted"))
        .and_then(Value::as_bool)
    {
        return trusted;
    }
    let answer = host.pi_call("ctx", "isProjectTrusted", vec![]).await;
    answer.ok().and_then(|v| v.as_bool()).unwrap_or(false)
}

pub async fn load_config(host: &Host, snapshot: Option<&Snapshot>) -> Loaded {
    let trusted = project_trusted(host, snapshot).await;
    let cwd = snapshot
        .map(|s| s.cwd_path())
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| host.cwd());
    let loaded = config::load(&host.agent_dir(), &cwd, trusted);
    if loaded.project_ignored && !IGNORED_NOTICE.swap(true, Ordering::Relaxed) {
        let path = config::project_path(&cwd);
        pi_ext_sdk::warn!(
            "web-search: ignoring {} because the project is not trusted",
            path.display()
        );
    }
    loaded
}

/// The full current model; the snapshot and `model.get` omit `baseUrl` and `headers`.
pub async fn current_model(host: &Host) -> Option<SearchModel> {
    let raw = host.pi_call("ctx", "model", vec![]).await.ok()?;
    SearchModel::from_json(raw)
}

pub async fn find_model(host: &Host, provider: &str, id: &str) -> Option<SearchModel> {
    let args = vec![json!(provider), json!(id)];
    let raw = host.pi_call("ctx.modelRegistry", "find", args).await.ok()?;
    SearchModel::from_json(raw)
}

/// `provider/id` descriptions of available models with native search.
pub async fn available_supported(host: &Host) -> Vec<String> {
    let Ok(Value::Array(models)) = host
        .pi_call("ctx.modelRegistry", "getAvailable", vec![])
        .await
    else {
        return Vec::new();
    };
    let models = models.into_iter().filter_map(SearchModel::from_json);
    models
        .filter(|m| m.kind() != Kind::Unsupported)
        .map(|m| m.describe())
        .collect()
}

pub async fn model_auth(host: &Host, model: &SearchModel) -> Result<Auth, String> {
    let answer = host
        .pi_call(
            "ctx.modelRegistry",
            "getApiKeyAndHeaders",
            vec![model.raw.clone()],
        )
        .await;
    let mut auth = match answer {
        Ok(value) => Auth::from_json(&value)?,
        Err(err) => return Err(err.message().to_owned()),
    };
    if auth.api_key.is_none() && !auth.has_auth_header() {
        auth.api_key = env_api_key(&model.provider);
    }
    Ok(auth)
}
