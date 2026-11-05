//! The `subscription-router` footer status: slot, percent left, and reset countdown.

use pi_ext_sdk::Host;

use crate::config::{ModelRef, ROUTER_PROVIDER};
use crate::family::parse_provider;
use crate::fsutil::now_ms;
use crate::slots::{SlotInfo, status_text};
use crate::state::{State, current_model, lock};

pub const STATUS_KEY: &str = "subscription-router";

fn slot_text(state: &State, model: &ModelRef) -> Option<String> {
    let (family, mut slot) = parse_provider(&model.provider)?;
    if slot == 1 && crate::pick::enabled(state) {
        slot = crate::pick::status_slot(state, family);
    }
    let model = &ModelRef::new(family.provider(slot), &model.id);
    let snapshot = state.usage().peek(&model.provider);
    Some(status_text(
        &SlotInfo {
            family,
            slot,
            snapshot,
        },
        now_ms(),
    ))
}

pub fn text(state: &State, current: Option<&ModelRef>) -> Option<String> {
    let current = current?;
    if current.provider != ROUTER_PROVIDER {
        let text = slot_text(state, current);
        let from = lock(&state.picks).fallback_from.clone();
        return match from.filter(|from| &from.to == current) {
            Some(from) => {
                let reason = format!("{} full, fallback", from.family.short());
                Some(text.map_or(reason.clone(), |t| format!("{t} · {reason}")))
            }
            None => text,
        };
    }
    let chosen = lock(&state.persisted).get(&current.id).cloned();
    let Some(chosen) = chosen.as_deref().and_then(ModelRef::parse) else {
        return Some(format!("router/{}", current.id));
    };
    let detail = slot_text(state, &chosen).map_or(String::new(), |t| format!(" · {t}"));
    Some(format!("router/{} → {}{detail}", current.id, chosen.key()))
}

pub async fn update(host: &Host, state: &State) {
    if !host.has_ui() {
        return;
    }
    let text = text(state, current_model(host).as_ref());
    host.set_status(STATUS_KEY, text.as_deref()).await.ok();
}
