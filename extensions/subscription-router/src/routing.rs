//! `router/<profile>` virtual models: the `virtualModel.route` handler and the
//! `agent_before_settle` recovery for quota errors pi does not retry.

use std::sync::Arc;
use std::time::Duration;

use pi_ext_sdk::RouteRequest;
use pi_ext_sdk::prelude::*;

use crate::config::{ModelRef, Profile, ROUTER_PROVIDER, THINKING_LEVELS};
use crate::fsutil::{iso_seconds, now_ms};
use crate::router::{FailedRequest, Pick, RouteError, RouteInput, Routed, can_wait};
use crate::state::{DECISION_ENTRY, HostEnv, State, current_model, lock};

pub fn definition(profile: &Profile) -> Value {
    json!({
        "provider": ROUTER_PROVIDER,
        "id": profile.id,
        "name": profile.name,
        "thinkingLevels": THINKING_LEVELS,
    })
}

fn chain_models(profile: &Profile) -> Vec<ModelRef> {
    profile.chain.iter().map(|e| e.model.clone()).collect()
}

async fn record(host: &Host, state: &State, routed: &Routed) {
    let Some(decision) = &routed.decision else {
        return;
    };
    if state.record(decision) {
        if let Err(err) = host.append_entry(DECISION_ENTRY, decision).await {
            pi_ext_sdk::warn!("cannot append {DECISION_ENTRY}: {err}");
        }
    }
}

/// Routes one request; with policy `wait` an exhausted chain sleeps until the earliest reset.
pub async fn route(state: Arc<State>, request: RouteRequest, host: Host) -> Result<Value> {
    let Some(profile) = state.config().profile(&request.id).cloned() else {
        bail!("subscription-router: unknown profile {}", request.id);
    };
    let input = RouteInput::from_json(&request.request);
    let mut waited = false;
    loop {
        let env = HostEnv::build(&host, &state, &chain_models(&profile)).await;
        match state.router.route(&profile, &input, &env).await {
            Ok(routed) => {
                record(&host, &state, &routed).await;
                crate::status::update(&host, &state).await;
                let model = &routed.entry.model;
                let route = json!({
                    "model": { "provider": model.provider, "id": model.id },
                    "thinkingLevel": routed.thinking_level,
                });
                return Ok(route);
            }
            Err(RouteError::Exhausted { message, reset_at }) => {
                let now = now_ms();
                let reset = reset_at
                    .filter(|_| !waited && can_wait(profile.exhaustion.as_ref(), reset_at, now));
                let Some(reset) = reset else {
                    bail!("{message}")
                };
                let notice = format!(
                    "router/{}: every model is exhausted; waiting until {}.",
                    profile.id,
                    iso_seconds(reset)
                );
                host.notify_warning(notice).await.ok();
                tokio::time::sleep(Duration::from_millis((reset - now).max(0) as u64 + 1_000))
                    .await;
                waited = true;
            }
            Err(error) => bail!("{}", error.message()),
        }
    }
}

/// The trailing failed assistant message of the projected context and its entry id.
pub fn trailing_error(event: &Value) -> Option<(String, Value)> {
    let entries = event.pointer("/context/contextEntries")?.as_array()?;
    for entry in entries.iter().rev() {
        let Some(last) = entry
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|m| m.last())
        else {
            continue;
        };
        let failed = last.get("role").and_then(Value::as_str) == Some("assistant")
            && last.get("stopReason").and_then(Value::as_str) == Some("error");
        if !failed {
            return None;
        }
        let id = entry.pointer("/sourceEntry/id")?.as_str()?.to_owned();
        return Some((id, last.clone()));
    }
    None
}

/// `agent_before_settle`: a confirmed quota error of a chain model drops the failed message with
/// a `context_edit` and continues; the continuation routes past the blocked model.
pub async fn on_before_settle(host: Host, state: Arc<State>, event: Value) -> Result<EventReply> {
    let Some(current) = current_model(&host).filter(|m| m.provider == ROUTER_PROVIDER) else {
        return Ok(EventReply::none());
    };
    let Some(profile) = state.config().profile(&current.id).cloned() else {
        return Ok(EventReply::none());
    };
    let Some((entry_id, message)) = trailing_error(&event) else {
        return Ok(EventReply::none());
    };
    let field = |k: &str| {
        message
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let failed =
        FailedRequest::from_message(ModelRef::new(field("provider"), field("model")), &message);
    if !failed.quota || failed.partial || profile.entry(&failed.model).is_none() {
        return Ok(EventReply::none());
    }
    if !lock(&state.recovered).insert(failed.model.key()) {
        return Ok(EventReply::none());
    }
    let env = HostEnv::build(&host, &state, &chain_models(&profile)).await;
    if !state
        .router
        .confirm(&failed.model, &env, failed.timestamp)
        .await
    {
        lock(&state.recovered).remove(&failed.model.key());
        return Ok(EventReply::none());
    }
    match state.router.pick(&profile, &env, None) {
        Pick::Chosen { entry, .. } => {
            let notice = format!(
                "subscription-router: {} is out of quota, continuing on {}",
                failed.model.key(),
                entry.model.key()
            );
            host.notify_warning(notice).await.ok();
            let mut entries = event
                .get("entries")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            entries
                .push(json!({ "type": "context_edit", "targetId": entry_id, "replacement": null }));
            Ok(EventReply::result(
                json!({ "entries": entries, "continue": true }),
            ))
        }
        Pick::Exhausted { blocked, reset_at } => {
            let message = crate::router::exhausted_message(&profile.id, &blocked, reset_at);
            host.notify_error(message).await.ok();
            Ok(EventReply::none())
        }
    }
}
