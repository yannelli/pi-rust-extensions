//! `/hybrid`: set up `hybrid/auto` with two selections, show the setup, or remove it.

use std::sync::Arc;

use pi_ext_sdk::prelude::*;

use crate::config::{self, Config, ModelRef};
use crate::route::{self, MODEL_ID, PROVIDER, Router};

/// The first pi-rust release whose `ctx.modelRegistry.findOfType` returns classifier models.
pub const CLASSIFIER_LOOKUP_PI: (u64, u64, u64) = (0, 12, 12);

pub async fn run(router: Arc<Router>, args: String, ctx: CommandCtx) -> Result<()> {
    let host = ctx.host().clone();
    match args.trim() {
        "" => setup(&router, &host).await,
        "status" => status(&router, &host).await,
        "off" => off(&router, &host).await,
        other => {
            let message = format!(
                "Unknown /hybrid argument \"{other}\". Use /hybrid, /hybrid status, or /hybrid off."
            );
            host.notify_warning(message).await
        }
    }
}

/// `major.minor.patch` of a version such as `0.12.12` or `v0.12.12-rc.1`.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    Some((
        parts.next()??,
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
    ))
}

/// Models of `ctx.modelRegistry.<method>(args)`, without virtual models.
async fn models(host: &Host, method: &str, args: Vec<Value>) -> Result<Vec<ModelRef>> {
    let listed = host.pi_call("ctx.modelRegistry", method, args).await?;
    let physical = |model: &&Value| model["api"] != "pi-virtual";
    Ok(listed
        .as_array()
        .into_iter()
        .flatten()
        .filter(physical)
        .filter_map(ModelRef::of)
        .collect())
}

fn no_local_model(host: &Host) -> String {
    let version = host.init().and_then(|init| parse_version(&init.pi_version));
    if version.is_some_and(|version| version < CLASSIFIER_LOOKUP_PI) {
        let (major, minor, patch) = CLASSIFIER_LOOKUP_PI;
        return format!(
            "hybrid-router needs pi {major}.{minor}.{patch} or later to find classifier models. Run pi update --self."
        );
    }
    "No model with a classifier is available. Start a llama.cpp router, run /login llama.cpp, and load a model with /llama."
        .into()
}

async fn setup(router: &Arc<Router>, host: &Host) -> Result<()> {
    let chat = models(host, "getAvailable", Vec::new()).await?;
    let classifiers = models(host, "getAvailableOfType", vec![json!("classifier")]).await?;
    let locals: Vec<String> = chat
        .iter()
        .filter(|model| classifiers.contains(model))
        .map(ToString::to_string)
        .collect();
    if locals.is_empty() {
        return host.notify_warning(no_local_model(host)).await;
    }
    let title = "Local model (rates each prompt and answers the simple ones)";
    let Some(local) = host
        .select(title, &locals)
        .await?
        .and_then(|text| ModelRef::parse(&text))
    else {
        return Ok(());
    };
    let remotes: Vec<ModelRef> = chat.into_iter().filter(|model| *model != local).collect();
    let mut providers: Vec<&str> = Vec::new();
    for model in &remotes {
        if !providers.contains(&model.provider.as_str()) {
            providers.push(&model.provider);
        }
    }
    let provider = match providers.as_slice() {
        [] => {
            return host
                .notify_warning("No other model is available. Log in to a provider with /login.")
                .await;
        }
        [only] => (*only).to_string(),
        _ => match host.select("Remote model provider", &providers).await? {
            Some(provider) => provider,
            None => return Ok(()),
        },
    };
    let ids: Vec<&str> = remotes
        .iter()
        .filter(|model| model.provider == provider)
        .map(|model| model.id.as_str())
        .collect();
    let title = format!("Remote model ({provider}) for complex prompts and busy periods");
    let Some(id) = host.select(&title, &ids).await? else {
        return Ok(());
    };
    let busy_load_per_core = router.config().and_then(|saved| saved.busy_load_per_core);
    let config = Config {
        local,
        remote: ModelRef { provider, id },
        busy_load_per_core,
    };
    let path = config::save(&host.agent_dir(), &config).map_err(Error::msg)?;
    router.set_config(Some(config.clone()));
    route::register(router, host).await?;
    if !host.set_model(PROVIDER, MODEL_ID).await? {
        host.notify_warning("Saved hybrid/auto, but pi did not select it. Select it with /model.")
            .await?;
    }
    let message = format!(
        "hybrid/auto: {} for simple prompts, {} for the others. Saved {}.",
        config.local,
        config.remote,
        path.display()
    );
    host.notify_info(message).await
}

async fn status(router: &Router, host: &Host) -> Result<()> {
    let Some(config) = router.config() else {
        return host
            .notify_info("hybrid/auto is not set up. Run /hybrid.")
            .await;
    };
    let busy = config
        .busy_load_per_core
        .unwrap_or(config::DEFAULT_BUSY_LOAD_PER_CORE);
    let load = route::load_per_core();
    let message = format!(
        "hybrid/auto: local {}, remote {}. Busy above {busy:.2} load per core (now {load:.2}).",
        config.local, config.remote
    );
    host.notify_info(message).await
}

/// Remove the config and `hybrid/auto`. A session on `hybrid/auto` moves to the remote model.
async fn off(router: &Router, host: &Host) -> Result<()> {
    let saved = router.config();
    config::remove(&host.agent_dir()).map_err(Error::msg)?;
    router.set_config(None);
    let current = host.model().await?;
    let selected = current.is_some_and(|model| model.provider == PROVIDER && model.id == MODEL_ID);
    if selected && let Some(saved) = &saved {
        host.set_model(&saved.remote.provider, &saved.remote.id)
            .await?;
    }
    route::unregister(router, host).await?;
    host.notify_info("Removed hybrid/auto.").await
}
