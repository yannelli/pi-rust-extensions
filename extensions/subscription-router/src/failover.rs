//! Slot failover outside `router/*` (multi-codex): a 429 on a slot switches the session to the
//! next eligible slot of the family and rewrites the error so pi retries on it.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use pi_ext_sdk::prelude::*;
use regex::Regex;

use crate::config::ModelRef;
use crate::family::{Family, parse_provider};
use crate::fsutil::now_ms;
use crate::slots::{failover_order, select_fresh, short_limited_until};
use crate::state::{State, current_model, lock};

static HTTP_429: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b429\b").expect("valid regex"));

#[derive(Default)]
pub struct Failover {
    /// The provider of the current model at `before_provider_request`, as multi-codex.
    pub request_provider: Option<String>,
    pub status: Option<u64>,
    pub tried: HashSet<u32>,
    /// Set while the extension itself switches models, so `model_select` is not a user choice.
    pub selecting: bool,
    pub explicit_choice: bool,
    pub opened: bool,
}

/// The family, failed slot, and model id of a 429 on the current slot model.
pub fn failed_slot(
    message: &Value,
    current: Option<&ModelRef>,
    status: Option<u64>,
) -> Option<(Family, u32, String)> {
    let get = |k: &str| message.get(k).and_then(Value::as_str);
    if get("role") != Some("assistant") || get("stopReason") != Some("error") {
        return None;
    }
    let error = get("errorMessage").unwrap_or_default();
    if status != Some(429) && !HTTP_429.is_match(error) {
        return None;
    }
    let (provider, id) = (get("provider")?, get("model")?);
    let current = current?;
    if current.provider != provider || current.id != id {
        return None;
    }
    let (family, slot) = parse_provider(provider)?;
    Some((family, slot, id.to_owned()))
}

pub fn retry_message(message: &Value) -> Value {
    let error = message.get("errorMessage").and_then(Value::as_str);
    let mut message = message.clone();
    let text = error.unwrap_or("request was rate limited.");
    message["errorMessage"] = json!(if text.contains("429") {
        text.to_owned()
    } else {
        format!("HTTP 429: {text}")
    });
    message
}

pub fn exhausted_message(message: &Value, family: Family) -> Value {
    let mut message = message.clone();
    message["errorMessage"] = json!(format!(
        "{} quota exceeded: all eligible account slots returned HTTP 429; automatic failover stopped.",
        family.label()
    ));
    message
}

pub async fn switch_to(host: &Host, state: &State, provider: &str, id: &str) -> bool {
    lock(&state.failover).selecting = true;
    let switched = host.set_model(provider, id).await.unwrap_or(false);
    lock(&state.failover).selecting = false;
    switched
}

/// `message_end`: returns `{ message }` with the rewritten error after a switch.
pub async fn on_message_end(host: Host, state: Arc<State>, event: Value) -> Result<EventReply> {
    let (provider, status) = {
        let mut failover = lock(&state.failover);
        (failover.request_provider.take(), failover.status.take())
    };
    let message = event.get("message").cloned().unwrap_or(Value::Null);
    let current = current_model(&host);
    let Some((family, failed, id)) = failed_slot(&message, current.as_ref(), status) else {
        return Ok(EventReply::none());
    };
    let message_provider = message.get("provider").and_then(Value::as_str);
    if !state.config().auto_switch_on_429
        || provider
            .as_deref()
            .is_some_and(|p| Some(p) != message_provider)
    {
        return Ok(EventReply::none());
    }
    let tried = {
        let mut failover = lock(&state.failover);
        failover.tried.insert(failed);
        failover.tried.clone()
    };
    for slot in failover_order(&state.slots(family), &tried, now_ms()) {
        if !switch_to(&host, &state, &family.provider(slot), &id).await {
            continue;
        }
        let notice = format!(
            "{} #{failed} returned HTTP 429. Retrying with {} #{slot}.",
            family.short(),
            family.short()
        );
        host.notify_warning(notice).await.ok();
        crate::status::update(&host, &state).await;
        return Ok(EventReply::result(
            json!({ "message": retry_message(&message) }),
        ));
    }
    Ok(EventReply::result(
        json!({ "message": exhausted_message(&message, family) }),
    ))
}

/// `before_agent_start`: on the first prompt, or when the current slot is five-hour limited,
/// move to the fresh slot with the most quota unless the user picked the model.
pub async fn pick_slot(host: &Host, state: &Arc<State>) {
    let Some(current) = current_model(host) else {
        return;
    };
    let Some((family, slot)) = parse_provider(&current.provider) else {
        return;
    };
    let now = now_ms();
    let slots = state.slots(family);
    let limited = slots
        .iter()
        .find(|s| s.slot == slot)
        .and_then(|s| s.snapshot.as_ref())
        .is_some_and(|s| short_limited_until(s, now).is_some());
    let (opened, explicit) = {
        let mut failover = lock(&state.failover);
        let opened = std::mem::replace(&mut failover.opened, true);
        (opened, failover.explicit_choice)
    };
    if (opened || explicit) && !limited {
        return;
    }
    if let Some(best) = select_fresh(&slots, Some(slot), now).filter(|best| *best != slot) {
        if switch_to(host, state, &family.provider(best), &current.id).await {
            crate::status::update(host, state).await;
        }
    }
}
