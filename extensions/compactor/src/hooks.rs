//! `context`, `input`, `session_start`, and settle hooks.

use std::sync::LazyLock;

use pi_ext_sdk::prelude::*;
use regex::Regex;

use crate::source::{COMPACTOR_INSTRUCTION, ORIGIN_ENTRY_TYPE, origin_key};
use crate::state::{COMPACT_THEN, QueueMode, Shared, spawn_compact};
use crate::text::str_of;

const LEGACY_HIDDEN_TYPES: [&str; 2] = ["compactor-auto-continue", "unipi-compactor-resume"];
static COMPACT_THEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/unipi:compact-then(\s+([\s\S]*))?$").unwrap());

pub async fn on_context(ev: EventCtx) -> Result<EventReply> {
    let Some(messages) = ev.get("messages").and_then(Value::as_array) else {
        return Ok(EventReply::none());
    };
    let hidden = |m: &Value| {
        str_of(m, "role") == "custom" && LEGACY_HIDDEN_TYPES.contains(&str_of(m, "customType"))
    };
    let kept: Vec<&Value> = messages.iter().filter(|m| !hidden(m)).collect();
    if kept.len() == messages.len() {
        return Ok(EventReply::none());
    }
    Ok(EventReply::result(json!({ "messages": kept })))
}

pub async fn on_input(ev: EventCtx, shared: Shared) -> Result<EventReply> {
    let host = ev.host().clone();
    let text = ev
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if ev.get("source").and_then(Value::as_str) == Some("extension") && !text.trim().is_empty() {
        let data = json!({ "key": origin_key(text.trim()) });
        if let Err(err) = host.append_entry(ORIGIN_ENTRY_TYPE, &data).await {
            pi_ext_sdk::debug!("origin mark failed: {}", err.message());
        }
    }
    let Some(caps) = COMPACT_THEN_RE.captures(&text) else {
        return Ok(EventReply::none());
    };
    let prompt = caps.get(2).map_or("", |m| m.as_str()).trim().to_owned();
    let mode = match ev.get("streamingBehavior").and_then(Value::as_str) {
        Some("followUp") => Some(QueueMode::FollowUp),
        Some("steer") => Some(QueueMode::Steer),
        _ => None,
    };
    let Some(mode) = mode else {
        shared.lock().set_pending(None, true, Some(COMPACT_THEN));
        spawn_compact(
            host,
            shared,
            Some(COMPACTOR_INSTRUCTION.into()),
            Some(prompt),
            false,
        );
        return Ok(EventReply::handled());
    };
    shared.lock().compact_then = Some((prompt, mode));
    let notice = match mode {
        QueueMode::FollowUp => "Compact then: queued — will compact and continue after this run.",
        QueueMode::Steer => "Compact then: queued — will compact and continue at the next turn.",
    };
    host.notify_info(notice).await.ok();
    Ok(EventReply::handled())
}

/// `agent_end` and `agent_settled`: run a compact-then queued as a follow-up.
pub async fn on_settled(ev: EventCtx, shared: Shared) -> Result<EventReply> {
    let queued = {
        let mut state = shared.lock();
        match state.compact_then.take() {
            Some((prompt, QueueMode::FollowUp)) => Some(prompt),
            other => {
                state.compact_then = other;
                None
            }
        }
    };
    if let Some(prompt) = queued {
        shared.lock().set_pending(None, true, Some(COMPACT_THEN));
        spawn_compact(
            ev.host().clone(),
            shared,
            Some(COMPACTOR_INSTRUCTION.into()),
            Some(prompt),
            false,
        );
    }
    Ok(EventReply::none())
}

pub async fn on_session_start(shared: Shared) -> Result<EventReply> {
    let mut state = shared.lock();
    state.auto = Default::default();
    state.set_pending(None, true, None);
    state.compact_then = None;
    Ok(EventReply::none())
}
