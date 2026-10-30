//! The `subscription_usage` tool, the `subscription_usage` prompt section, and the
//! `router:rpc:*` handlers on the `pi.events` bus.

use std::sync::Arc;

use pi_ext_sdk::prelude::*;
use serde::Deserialize;

use crate::config::ModelRef;
use crate::fsutil::now_ms;
use crate::state::{HostEnv, HostTokens, State, current_model, lock, warm};
use crate::usage::{GetOptions, format, format_for_prompt};

pub const PROMPT_SECTION: &str = "subscription_usage";
pub const RPC_ADMIT: &str = "router:rpc:admit";
pub const RPC_USAGE: &str = "router:rpc:usage";

pub fn spec() -> ToolSpec {
    ToolSpec::new(
        "subscription_usage",
        "Inspect subscription quota windows and the router's confirmed exhaustion blocks. Exhausted families are skipped by router/* models until they reset.",
    )
    .label("Subscription Usage")
    .prompt_snippet("Inspect subscription quota before expensive subagent fan-out")
    .parameters(json!({
        "type": "object",
        "properties": {
            "provider": { "type": "string", "description": "Provider to inspect. Omit for every relevant provider." },
            "refresh": { "type": "boolean", "description": "Request a live refresh. Limited to once per minute." }
        }
    }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct UsageArgs {
    pub provider: Option<String>,
    pub refresh: Option<bool>,
}

fn current_provider(host: &Host) -> Option<String> {
    current_model(host).map(|m| m.provider)
}

pub async fn run(state: Arc<State>, args: UsageArgs, host: Host) -> Result<ToolResult> {
    let providers = match args.provider.filter(|p| !p.is_empty()) {
        Some(provider) => vec![provider],
        None => state.session_providers(current_provider(&host).as_deref()),
    };
    let options = GetOptions {
        force: args.refresh.unwrap_or(false),
        ..Default::default()
    };
    let tokens = HostTokens {
        host: &host,
        state: &state,
    };
    let mut snapshots = Vec::new();
    for provider in &providers {
        snapshots.push(state.usage().get(provider, &tokens, options).await);
    }
    let mut lines: Vec<String> = snapshots
        .iter()
        .map(|s| format!("{}: {}", s.provider, format(Some(s))))
        .collect();
    let blocks = state.router.tracker().active(now_ms());
    for mark in &blocks {
        let scope = match &mark.families {
            Some(families) => format!("{} {}", mark.provider, families.join(",")),
            None => mark.provider.clone(),
        };
        let until = chrono::DateTime::from_timestamp_millis(mark.until).map(|t| t.to_rfc3339());
        lines.push(format!(
            "blocked {scope}: {} until {}",
            mark.window,
            until.unwrap_or_default()
        ));
    }
    let text = if lines.is_empty() {
        "No subscription providers in use.".to_owned()
    } else {
        lines.join("\n")
    };
    Ok(ToolResult::text(text).details(json!({ "snapshots": snapshots, "blocks": blocks })))
}

/// Cached data only, in coarse buckets, so the prompt prefix stays stable.
pub fn prompt_section(state: &State, current: Option<&str>) -> Option<String> {
    let providers = state.session_providers(current);
    if providers.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = providers
        .iter()
        .map(|p| {
            format!(
                "- {p}: {}",
                format_for_prompt(state.usage().peek(p).as_ref())
            )
        })
        .collect();
    lines.push("- Treat usage as decision support. Check subscription_usage before subagent fan-out and prefer cheaper models for bounded reconnaissance when capacity is low.".into());
    Some(lines.join("\n"))
}

pub async fn before_agent_start(host: Host, state: Arc<State>) -> Result<EventReply> {
    lock(&state.recovered).clear();
    lock(&state.failover).tried.clear();
    crate::failover::pick_slot(&host, &state).await;
    let current = current_provider(&host);
    warm(&host, &state, state.session_providers(current.as_deref()));
    Ok(match prompt_section(&state, current.as_deref()) {
        Some(section) => EventReply::section(PROMPT_SECTION, section),
        None => EventReply::none(),
    })
}

async fn admit(host: &Host, state: &Arc<State>, params: &Value) -> Result<Value> {
    let model = params
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let config = state.config();
    let models: Vec<ModelRef> = config
        .profiles
        .iter()
        .flat_map(|p| p.chain.iter().map(|e| e.model.clone()))
        .collect();
    let env = HostEnv::build(host, state, &models).await;
    Ok(serde_json::to_value(
        state.router.admit(model, &config, &env),
    )?)
}

async fn usage(host: &Host, state: &Arc<State>, params: &Value) -> Result<Value> {
    if params.get("refresh").and_then(Value::as_bool) == Some(true) {
        let tokens = HostTokens { host, state };
        for provider in state.session_providers(current_provider(host).as_deref()) {
            state
                .usage()
                .get(&provider, &tokens, GetOptions::default())
                .await;
        }
    }
    let blocks = state.router.tracker().active(now_ms());
    Ok(json!({ "snapshots": state.usage().snapshots(), "blocks": blocks }))
}

/// Serves one RPC channel: requests carry `requestId`; replies go to `<channel>:reply:<requestId>`.
pub async fn serve_rpc(host: Host, state: Arc<State>, channel: &'static str) -> Result<()> {
    let mut subscription = host.subscribe(channel).await?;
    while let Some(params) = subscription.recv().await {
        let Some(request_id) = params
            .get("requestId")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            continue;
        };
        let (host, state) = (host.clone(), state.clone());
        tokio::spawn(async move {
            let result = match channel {
                RPC_ADMIT => admit(&host, &state, &params).await,
                _ => usage(&host, &state, &params).await,
            };
            let reply = match result {
                Ok(data) => json!({ "success": true, "data": data }),
                Err(err) => json!({ "success": false, "error": err.message() }),
            };
            let reply_channel = format!("{channel}:reply:{request_id}");
            if let Err(err) = host.emit(&reply_channel, reply).await {
                pi_ext_sdk::warn!("cannot reply on {reply_channel}: {err}");
            }
        });
    }
    Ok(())
}
