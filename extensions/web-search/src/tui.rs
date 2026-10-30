//! `/web-search`: choose providers, the search model, base URLs, and API keys.

use std::path::PathBuf;

use pi_ext_sdk::prelude::*;

use crate::config::{Api, Choice, Loaded, global_path, project_path, update_file};
use crate::context::load_config;
use crate::keys::{KeySource, api_key, keys_path, mask, set_key};
use crate::toolsync::{Shared, sync};

const CHOICES: [&str; 6] = ["auto", "native", "exa", "firecrawl", "brave", "tavily"];

async fn pick(host: &Host, title: &str, options: &[String]) -> Result<Option<usize>> {
    let answer = host.select(title, options).await?;
    Ok(answer.and_then(|a| options.iter().position(|o| *o == a)))
}

fn key_status(agent_dir: &std::path::Path, api: Api) -> String {
    match api_key(agent_dir, api) {
        Some((key, KeySource::Stored)) => format!("stored {}", mask(&key)),
        Some((_, KeySource::Env)) => format!("from {}", api.env()),
        None => "not set".to_owned(),
    }
}

/// Global or project file; `None` when dismissed.
async fn scope(host: &Host, loaded: &Loaded) -> Result<Option<PathBuf>> {
    let (global, project) = (global_path(&host.agent_dir()), project_path(&host.cwd()));
    let note = if loaded.project_ignored {
        " (untrusted, ignored)"
    } else {
        ""
    };
    let options = vec![
        format!("Global: {}", global.display()),
        format!("Project: {}{note}", project.display()),
    ];
    Ok(pick(host, "Save to", &options)
        .await?
        .map(|i| if i == 0 { global } else { project }))
}

async fn write(host: &Host, loaded: &Loaded, patch: Value) -> Result<()> {
    let Some(path) = scope(host, loaded).await? else {
        return Ok(());
    };
    update_file(&path, &patch)?;
    host.notify_info(format!("Saved {}", path.display())).await
}

async fn choose_provider(host: &Host, loaded: &Loaded, tool: Option<&str>) -> Result<()> {
    let mut options: Vec<String> = CHOICES.iter().map(|c| c.to_string()).collect();
    if tool.is_some() {
        options.insert(0, "inherit default".to_owned());
    }
    let title = tool.map_or_else(
        || "Default provider".to_owned(),
        |t| format!("{t} provider"),
    );
    let Some(index) = pick(host, &title, &options).await? else {
        return Ok(());
    };
    let value = if options[index] == "inherit default" {
        Value::Null
    } else {
        json!(options[index])
    };
    let patch = match tool {
        Some(tool) => json!({ "tools": { tool: value } }),
        None => json!({ "defaultProvider": value }),
    };
    write(host, loaded, patch).await
}

async fn search_model(host: &Host, loaded: &Loaded) -> Result<()> {
    let Some(text) = host
        .input(
            "Search model (provider/id, empty clears)",
            Some("openai/gpt-5.1"),
        )
        .await?
    else {
        return Ok(());
    };
    let text = text.trim();
    if !text.is_empty()
        && !text
            .split_once('/')
            .is_some_and(|(p, i)| !p.is_empty() && !i.is_empty())
    {
        return host
            .notify_error("Use provider/id, for example openai/gpt-5.1")
            .await;
    }
    let value = if text.is_empty() {
        Value::Null
    } else {
        json!(text)
    };
    write(host, loaded, json!({ "searchModel": value })).await
}

async fn pick_api(host: &Host, title: &str, detail: impl Fn(Api) -> String) -> Result<Option<Api>> {
    let options: Vec<String> = Api::ALL
        .iter()
        .map(|api| format!("{}: {}", api.label(), detail(*api)))
        .collect();
    Ok(pick(host, title, &options).await?.map(|i| Api::ALL[i]))
}

async fn keys_menu(host: &Host) -> Result<()> {
    let agent_dir = host.agent_dir();
    let Some(api) = pick_api(host, "API keys", |api| key_status(&agent_dir, api)).await? else {
        return Ok(());
    };
    let options = vec!["Set key".to_owned(), "Remove stored key".to_owned()];
    match pick(host, api.label(), &options).await? {
        Some(0) => {
            let Some(key) = host
                .input(&format!("{} API key", api.label()), None)
                .await?
            else {
                return Ok(());
            };
            if key.trim().is_empty() {
                return Ok(());
            }
            set_key(&agent_dir, api, Some(&key))?;
            host.notify_info(format!(
                "Stored {} key in {}",
                api.label(),
                keys_path(&agent_dir).display()
            ))
            .await
        }
        Some(_) => {
            set_key(&agent_dir, api, None)?;
            host.notify_info(format!("Removed stored {} key", api.label()))
                .await
        }
        None => Ok(()),
    }
}

async fn base_url(host: &Host, loaded: &Loaded) -> Result<()> {
    let config = &loaded.config;
    let Some(api) = pick_api(host, "Base URL", |api| config.base_url(api)).await? else {
        return Ok(());
    };
    let prompt = format!(
        "{} base URL (empty resets to {})",
        api.label(),
        api.default_base_url()
    );
    let Some(url) = host.input(&prompt, Some(api.default_base_url())).await? else {
        return Ok(());
    };
    let url = url.trim().trim_end_matches('/');
    let value = if url.is_empty() {
        Value::Null
    } else {
        json!(url)
    };
    write(
        host,
        loaded,
        json!({ "providers": { api.id(): { "baseUrl": value } } }),
    )
    .await
}

fn shown(loaded: &Loaded, tool: Option<&str>) -> String {
    let config = &loaded.config;
    match tool {
        None => config
            .default_provider
            .clone()
            .unwrap_or_else(|| Choice::Auto.id().to_owned()),
        Some(tool) => config
            .tools
            .get(tool)
            .cloned()
            .unwrap_or_else(|| "inherit default".to_owned()),
    }
}

pub async fn run(host: Host, state: Shared) -> Result<()> {
    if !host.has_ui() {
        let loaded = load_config(&host, host.snapshot().as_deref()).await;
        let text = format!(
            "web-search provider: {} (edit {})",
            shown(&loaded, None),
            global_path(&host.agent_dir()).display()
        );
        return host.notify_info(text).await;
    }
    loop {
        let snapshot = host.snapshot();
        let loaded = load_config(&host, snapshot.as_deref()).await;
        if let Some((path, error)) = &loaded.invalid {
            host.notify_error(format!("{}: {error}", path.display()))
                .await?;
        }
        let model = loaded
            .config
            .search_model
            .clone()
            .unwrap_or_else(|| "current model".to_owned());
        let options = vec![
            format!("Default provider: {}", shown(&loaded, None)),
            format!(
                "web_search provider: {}",
                shown(&loaded, Some("web_search"))
            ),
            format!(
                "url_context provider: {}",
                shown(&loaded, Some("url_context"))
            ),
            format!("Search model: {model}"),
            "API keys".to_owned(),
            "Base URLs".to_owned(),
            "Done".to_owned(),
        ];
        let step = match pick(&host, "Web search", &options).await? {
            Some(0) => choose_provider(&host, &loaded, None).await,
            Some(1) => choose_provider(&host, &loaded, Some("web_search")).await,
            Some(2) => choose_provider(&host, &loaded, Some("url_context")).await,
            Some(3) => search_model(&host, &loaded).await,
            Some(4) => keys_menu(&host).await,
            Some(5) => base_url(&host, &loaded).await,
            _ => break,
        };
        if let Err(err) = step {
            if err.is_cancelled() {
                break;
            }
            host.notify_error(err.message().to_owned()).await?;
        }
    }
    if let Some(snapshot) = host.snapshot() {
        sync(&state, &host, &snapshot, None).await?;
    }
    Ok(())
}
