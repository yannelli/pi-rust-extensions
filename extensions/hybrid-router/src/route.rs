//! Routing of `hybrid/auto`: each new prompt goes to the local model when its classifier rates the
//! prompt simple and the machine is not busy, and to the remote model otherwise.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use pi_ext_sdk::RouteRequest;
use pi_ext_sdk::prelude::*;

use crate::config::{Config, ModelRef};

pub const PROVIDER: &str = "hybrid";
pub const MODEL_ID: &str = "auto";
pub const QUESTION: &str = "complexity";
const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
/// The classifier reads at most this many characters of the prompt.
const PROMPT_CHARS: usize = 8_000;

pub fn definition() -> Value {
    json!({ "provider": PROVIDER, "id": MODEL_ID, "name": "Hybrid Auto", "thinkingLevels": THINKING_LEVELS })
}

/// The config, whether `hybrid/auto` is registered, and the load probe.
pub struct Router {
    config: Mutex<Option<Config>>,
    pub registered: AtomicBool,
    load_per_core: fn() -> f64,
}

impl Router {
    pub fn new(config: Option<Config>, load_per_core: fn() -> f64) -> Self {
        let registered = AtomicBool::new(config.is_some());
        Self {
            config: Mutex::new(config),
            registered,
            load_per_core,
        }
    }

    pub fn config(&self) -> Option<Config> {
        self.config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn set_config(&self, config: Option<Config>) {
        *self
            .config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = config;
    }
}

/// Register `hybrid/auto` unless it is registered.
pub async fn register(router: &Arc<Router>, host: &Host) -> Result<()> {
    if router.registered.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let routes = router.clone();
    host.register_virtual_model(definition(), move |request, host| {
        route(routes.clone(), request, host)
    })
    .await
}

pub async fn unregister(router: &Router, host: &Host) -> Result<()> {
    if router.registered.swap(false, Ordering::SeqCst) {
        host.unregister_virtual_model(PROVIDER, MODEL_ID).await?;
    }
    Ok(())
}

/// The 1-minute load average divided by the number of cores; 0 when the system does not report it.
pub fn load_per_core() -> f64 {
    let cores = std::thread::available_parallelism().map_or(1, |count| count.get()) as f64;
    one_minute_load().map_or(0.0, |load| load / cores)
}

/// From `/proc/loadavg` on Linux and `sysctl vm.loadavg` (`{ 1.52 1.40 1.31 }`) on macOS and BSD.
fn one_minute_load() -> Option<f64> {
    let text = match std::fs::read_to_string("/proc/loadavg") {
        Ok(text) => text,
        Err(_) => {
            let output = std::process::Command::new("sysctl")
                .args(["-n", "vm.loadavg"])
                .output()
                .ok()?;
            String::from_utf8_lossy(&output.stdout).into_owned()
        }
    };
    text.split_whitespace().find_map(|field| field.parse().ok())
}

/// Text of the latest user message of a request's `messages`.
pub fn latest_prompt(messages: &Value) -> String {
    let latest = messages
        .as_array()
        .into_iter()
        .flatten()
        .rev()
        .find(|message| message["role"] == "user");
    let Some(content) = latest.map(|message| &message["content"]) else {
        return String::new();
    };
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => {
            let texts = blocks
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str());
            texts.collect::<Vec<_>>().join("\n")
        }
        _ => String::new(),
    };
    text.chars().take(PROMPT_CHARS).collect()
}

/// The classifier context: one `choice` question about `prompt`.
pub fn complexity_context(prompt: &str) -> Value {
    json!({
        "state": { "prompt": prompt },
        "questions": { QUESTION: {
            "type": "choice",
            "instructions": "How demanding is the software engineering work that `prompt` asks for?",
            "criteria": {
                "simple": "Quick questions, small fixes, lookups",
                "complex": "Architecture, changes across many files, hard debugging",
            },
        } },
    })
}

/// The probability of `complex` in a classifier result; `None` when the classification failed.
pub fn complex_probability(result: &Value) -> Option<f64> {
    if result["stopReason"] != "stop" {
        return None;
    }
    result["answers"][QUESTION]["probabilities"]["complex"].as_f64()
}

/// Whether the classifier of `local` rates `prompt` complex. A prompt it cannot rate counts as
/// complex, also when pi has no classifier with the provider and id of `local`.
async fn is_complex(host: &Host, local: &ModelRef, prompt: &str) -> bool {
    let args = vec![json!("classifier"), json!(local.provider), json!(local.id)];
    let classifier = match host.pi_call("ctx.modelRegistry", "findOfType", args).await {
        Ok(model) if model.is_object() => model,
        _ => return true,
    };
    let args = vec![classifier, complexity_context(prompt), json!({})];
    match host.pi_call("ctx.modelRegistry", "classify", args).await {
        Ok(result) => complex_probability(&result).is_none_or(|probability| probability >= 0.5),
        Err(_) => true,
    }
}

/// The route reply; without a thinking level, pi uses the selected one.
fn reply(model: &ModelRef, thinking_level: Option<&str>) -> Value {
    let mut route = json!({ "model": { "provider": model.provider, "id": model.id } });
    if let Some(level) = thinking_level {
        route["thinkingLevel"] = json!(level);
    }
    route
}

/// Routes one request. Requests that continue a turn, such as after tool results, stay on the
/// model that answered. A failed local request retries on the remote model.
pub async fn route(router: Arc<Router>, request: RouteRequest, host: Host) -> Result<Value> {
    let Some(config) = router.config() else {
        bail!("hybrid/auto is not set up. Run /hybrid.");
    };
    let request = &request.request;
    let failed = &request["failed"];
    if let Some(model) = ModelRef::of(&failed["model"]) {
        if model == config.local {
            return Ok(reply(&config.remote, None));
        }
        return Ok(reply(&model, failed["thinkingLevel"].as_str()));
    }
    let previous = &request["previous"];
    if request["reason"] != "user"
        && let Some(model) = ModelRef::of(&previous["model"])
    {
        return Ok(reply(&model, previous["thinkingLevel"].as_str()));
    }
    let local_found = host
        .find_model(&config.local.provider, &config.local.id)
        .await;
    let local = matches!(local_found, Ok(Some(_)))
        && !config.busy((router.load_per_core)())
        && !is_complex(&host, &config.local, &latest_prompt(&request["messages"])).await;
    Ok(reply(
        if local { &config.local } else { &config.remote },
        None,
    ))
}
