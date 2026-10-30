//! Picks the provider for a tool call, or the upstream-shaped error result when none fits.

use std::path::Path;

use pi_ext_sdk::{Host, Snapshot, ToolResult};
use serde_json::json;

use crate::api::Endpoint;
use crate::config::{Api, Choice, Legacy, Loaded, global_path};
use crate::context::{available_supported, current_model, find_model, model_auth};
use crate::format::{error_result, exception_result};
use crate::keys::api_key;
use crate::model::{SUPPORTED_PROVIDERS, SearchModel, Target};

pub enum Plan {
    Native(Box<Target>),
    Api(Endpoint),
    Fail(ToolResult),
}

const KEY_HINT: &str = " Or add an Exa, Tavily, Brave, or Firecrawl API key with /web-search.";

pub fn endpoint(agent_dir: &Path, loaded: &Loaded, api: Api) -> Option<Endpoint> {
    let (key, _) = api_key(agent_dir, api)?;
    let config = &loaded.config;
    Some(Endpoint {
        api,
        base_url: config.base_url(api),
        key,
        max_results: config.max_results(api),
    })
}

fn first_keyed(agent_dir: &Path, loaded: &Loaded, fetch: bool) -> Option<Endpoint> {
    let apis = Api::ALL
        .into_iter()
        .filter(|api| !fetch || api.fetches_urls());
    apis.into_iter()
        .find_map(|api| endpoint(agent_dir, loaded, api))
}

fn missing_key(api: Api) -> Plan {
    let text = format!(
        "Failed: No {} API key. Set {} or run /web-search to store a key.",
        api.label(),
        api.env()
    );
    Plan::Fail(error_result(
        &text,
        json!({ "error": "missing_api_key", "provider": api.id() }),
    ))
}

fn invalid_config(path: &Path, error: &str) -> Plan {
    let path = path.display();
    let text = format!(
        "Failed: Invalid web search model config at {path}: {error}. Fix the file or remove it to use the current conversation model."
    );
    let details = json!({ "error": "invalid_config", "configPath": path.to_string(), "configError": error, "supportedProviders": SUPPORTED_PROVIDERS });
    Plan::Fail(error_result(&text, details))
}

fn api_plan(agent_dir: &Path, loaded: &Loaded, api: Api) -> Plan {
    endpoint(agent_dir, loaded, api).map_or_else(|| missing_key(api), Plan::Api)
}

async fn target(host: &Host, snapshot: &Snapshot, model: SearchModel) -> Plan {
    match model_auth(host, &model).await {
        Ok(auth) => Plan::Native(Box::new(Target {
            model,
            auth,
            session_id: snapshot.session_id.clone(),
            thinking_level: snapshot.thinking_level.clone(),
        })),
        Err(error) => Plan::Fail(exception_result(&error)),
    }
}

async fn unsupported_current(host: &Host, model: Option<&SearchModel>, hint: &str) -> Plan {
    let available = available_supported(host).await;
    let advice = if available.is_empty() {
        format!(
            " Configure and select a supported provider: {}.",
            SUPPORTED_PROVIDERS.join(", ")
        )
    } else {
        format!(
            " Select one of these configured supported models manually: {}.",
            available.join(", ")
        )
    };
    let (text, details) = match model {
        Some(model) => (
            format!(
                "The current model {} does not support native web search. pi-web-search will not switch to another configured model automatically to avoid unexpected API costs.{advice}{hint}",
                model.describe()
            ),
            json!({ "error": "unsupported_model", "currentModel": model.describe(), "availableSupportedModels": available, "supportedProviders": SUPPORTED_PROVIDERS }),
        ),
        None => (
            format!("No current model selected for web search.{advice}{hint}"),
            json!({ "error": "missing_config", "availableSupportedModels": available, "supportedProviders": SUPPORTED_PROVIDERS }),
        ),
    };
    Plan::Fail(error_result(&format!("Failed: {text}"), details))
}

/// The configured search model: `searchModel` in the config files, else the legacy file.
fn configured_model(
    loaded: &Loaded,
    agent_dir: &Path,
) -> Result<Option<(String, String, String)>, Plan> {
    if let Some(spec) = loaded
        .config
        .search_model
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        let path = global_path(agent_dir).display().to_string();
        return match spec.trim().split_once('/') {
            Some((provider, id)) if !provider.is_empty() && !id.is_empty() => {
                Ok(Some((path, provider.to_owned(), id.to_owned())))
            }
            _ => Err(invalid_config(
                &global_path(agent_dir),
                "searchModel must be \"provider/id\"",
            )),
        };
    }
    match &loaded.legacy {
        Legacy::Missing => Ok(None),
        Legacy::Invalid { path, error } => Err(invalid_config(path, error)),
        Legacy::Model {
            path,
            provider,
            model,
        } => Ok(Some((
            path.display().to_string(),
            provider.clone(),
            model.clone(),
        ))),
    }
}

async fn native_search(host: &Host, snapshot: &Snapshot, loaded: &Loaded, hint: &str) -> Plan {
    let configured = match configured_model(loaded, &host.agent_dir()) {
        Ok(configured) => configured,
        Err(plan) => return plan,
    };
    let Some((path, provider, id)) = configured else {
        return match current_model(host).await {
            Some(model) if model.kind() != crate::model::Kind::Unsupported => {
                target(host, snapshot, model).await
            }
            model => unsupported_current(host, model.as_ref(), hint).await,
        };
    };
    let available = available_supported(host).await;
    let Some(model) = find_model(host, &provider, &id).await else {
        let text = format!(
            "Failed: Configured web search model {provider}/{id} from {path} was not found. Fix the file or remove it to use the current conversation model."
        );
        let details = json!({ "error": "configured_model_not_found", "configPath": path, "configuredProvider": provider, "configuredModel": id, "availableSupportedModels": available, "supportedProviders": SUPPORTED_PROVIDERS });
        return Plan::Fail(error_result(&text, details));
    };
    if model.kind() == crate::model::Kind::Unsupported {
        let text = format!(
            "Failed: Configured web search model {} from {path} does not support native web search. Configure a model backed by {}, or remove the file to use the current conversation model.",
            model.describe(),
            SUPPORTED_PROVIDERS.join(", ")
        );
        let details = json!({ "error": "unsupported_model", "configPath": path, "configuredModel": model.describe(), "configuredProvider": provider, "availableSupportedModels": available, "supportedProviders": SUPPORTED_PROVIDERS });
        return Plan::Fail(error_result(&text, details));
    }
    target(host, snapshot, model).await
}

fn choice(loaded: &Loaded, agent_dir: &Path, tool: &str) -> Result<Choice, Plan> {
    if let Some((path, error)) = &loaded.invalid {
        return Err(invalid_config(path, error));
    }
    loaded
        .config
        .choice(tool)
        .map_err(|error| invalid_config(&global_path(agent_dir), &error))
}

pub async fn web_search(host: &Host, snapshot: &Snapshot, loaded: &Loaded) -> Plan {
    let agent_dir = host.agent_dir();
    match choice(loaded, &agent_dir, "web_search") {
        Err(plan) => plan,
        Ok(Choice::Api(api)) => api_plan(&agent_dir, loaded, api),
        Ok(Choice::Native) => native_search(host, snapshot, loaded, "").await,
        Ok(Choice::Auto) => {
            let explicit = loaded.config.search_model.is_some() || loaded.legacy != Legacy::Missing;
            let fallback = first_keyed(&agent_dir, loaded, false);
            match (explicit, fallback) {
                (false, Some(endpoint)) => match current_model(host).await {
                    Some(model) if model.kind() != crate::model::Kind::Unsupported => {
                        target(host, snapshot, model).await
                    }
                    _ => Plan::Api(endpoint),
                },
                (explicit, _) => {
                    native_search(host, snapshot, loaded, if explicit { "" } else { KEY_HINT })
                        .await
                }
            }
        }
    }
}

fn unsupported_url_context(model: Option<&SearchModel>, provider: &str) -> Plan {
    let current = model.map_or_else(
        || "none".to_owned(),
        |m| format!("{} ({}/{})", m.id, m.provider, m.api),
    );
    let text = format!(
        "url_context currently requires a Google Gemini-compatible model. Current model: {current}.\n\nUse web_search for cross-provider web search, or switch to Gemini for provider-native URL context retrieval. Exa, Tavily, and Firecrawl keys also enable url_context through /web-search."
    );
    let details = json!({ "error": "unsupported_provider", "providerKind": provider, "model": model.map(|m| m.id.clone()), "supportedProviders": ["google", "google-generative-ai"], "grounded": false });
    Plan::Fail(error_result(&text, details))
}

pub async fn url_context(host: &Host, snapshot: &Snapshot, loaded: &Loaded) -> Plan {
    let agent_dir = host.agent_dir();
    let choice = match choice(loaded, &agent_dir, "url_context") {
        Ok(choice) => choice,
        Err(plan) => return plan,
    };
    if let Choice::Api(api) = choice {
        return if api.fetches_urls() {
            api_plan(&agent_dir, loaded, api)
        } else {
            unsupported_url_context(None, api.id())
        };
    }
    let model = current_model(host).await;
    match model {
        Some(model) if model.supports_url_context() => target(host, snapshot, model).await,
        None if choice == Choice::Native => unsupported_current(host, None, "").await,
        model => match (choice, first_keyed(&agent_dir, loaded, true)) {
            (Choice::Auto, Some(endpoint)) => Plan::Api(endpoint),
            _ => unsupported_url_context(
                model.as_ref(),
                model.as_ref().map_or("unknown", |m| m.kind().id()),
            ),
        },
    }
}

/// Whether `url_context` can run without a network check; drives the active tool list.
pub fn url_context_available(
    agent_dir: &Path,
    loaded: &Loaded,
    model: Option<&SearchModel>,
) -> bool {
    let native = model.is_some_and(SearchModel::supports_url_context);
    match loaded.config.choice("url_context") {
        Ok(Choice::Native) => native,
        Ok(Choice::Api(api)) => api.fetches_urls() && endpoint(agent_dir, loaded, api).is_some(),
        Ok(Choice::Auto) => native || first_keyed(agent_dir, loaded, true).is_some(),
        Err(_) => false,
    }
}
