use std::sync::Arc;

use pi_ext_sdk::event_types::InputEvent;
use pi_ext_sdk::prelude::*;

use crate::decide::{contexts, first_block, first_stop, stop_text};
use crate::dispatch::{Dispatch, dispatch};
use crate::events::tool::content_text;
use crate::matcher::Target;
use crate::runtime::Runtime;

pub async fn input(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let event: InputEvent = ev.event_as()?;
    if event.source == "extension" {
        return Ok(EventReply::none());
    }
    {
        let mut state = rt.lock();
        state.stop_hook_active = false;
        state.turn += 1;
    }
    let d = Dispatch::new(
        "UserPromptSubmit",
        Target::Any,
        json!({ "prompt": event.text }),
    );
    let outcomes = dispatch(&rt, ev.host(), ev.snapshot(), d).await;
    if let Some(reason) = first_stop(&outcomes)
        .map(|r| stop_text(&r))
        .or_else(|| first_block(&outcomes))
    {
        let _ = ev
            .host()
            .notify_warning(format!("Prompt blocked by hook: {reason}"))
            .await;
        return Ok(EventReply::handled());
    }
    let context = contexts(&outcomes);
    if context.is_empty() {
        return Ok(EventReply::none());
    }
    Ok(EventReply::transform(format!(
        "{}\n\n{}",
        event.text,
        context.join("\n\n")
    )))
}

pub async fn agent_end(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let messages = ev
        .event()
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let last = messages
        .iter()
        .rev()
        .find(|m| m.get("role").and_then(Value::as_str) == Some("assistant"));
    let mut state = rt.lock();
    state.last_assistant = last.map(|m| {
        let content = m
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        content_text(&content)
    });
    state.last_error = last
        .filter(|m| m.get("stopReason").and_then(Value::as_str) == Some("error"))
        .map(|m| {
            m.get("errorMessage")
                .and_then(Value::as_str)
                .unwrap_or("error")
                .to_owned()
        });
    Ok(EventReply::none())
}

/// Claude StopFailure matcher values from an error message.
pub fn error_type(message: &str) -> &'static str {
    let text = message.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| text.contains(w));
    if has(&["rate limit", "rate_limit", "429"]) {
        "rate_limit"
    } else if has(&["401", "403", "unauthorized", "authentication", "api key"]) {
        "authentication_failed"
    } else if has(&["billing", "quota", "credit"]) {
        "billing_error"
    } else if has(&["max_tokens", "max output", "maximum output"]) {
        "max_output_tokens"
    } else if has(&["500", "502", "503", "529", "overloaded", "server error"]) {
        "server_error"
    } else if has(&["400", "invalid request", "invalid_request"]) {
        "invalid_request"
    } else {
        "unknown"
    }
}

pub async fn agent_settled(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let (host, snap) = (ev.host(), ev.snapshot());
    if ev
        .event()
        .get("aborted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let mut d = Dispatch::new("Interrupt", Target::Any, json!({}));
        d.claude = None;
        dispatch(&rt, host, snap, d).await;
        return Ok(EventReply::none());
    }
    let (active, last, error) = {
        let state = rt.lock();
        (
            state.stop_hook_active,
            state.last_assistant.clone(),
            state.last_error.clone(),
        )
    };
    let payload = json!({ "stop_hook_active": active, "last_assistant_message": last });
    let outcomes = match error {
        Some(error) => {
            let kind = error_type(&error);
            let mut failure = payload.clone();
            failure["error"] = json!(kind);
            failure["error_details"] = json!(error);
            let claude =
                Dispatch::new("StopFailure", Target::Value(kind.into()), failure).claude_only();
            dispatch(&rt, host, snap, claude).await;
            let mut codex = Dispatch::new("Stop", Target::Any, payload);
            codex.claude = None;
            dispatch(&rt, host, snap, codex).await
        }
        None => dispatch(&rt, host, snap, Dispatch::new("Stop", Target::Any, payload)).await,
    };
    if let Some(reason) = first_stop(&outcomes) {
        let _ = host.notify_info(stop_text(&reason)).await;
        return Ok(EventReply::none());
    }
    if let Some(reason) = first_block(&outcomes) {
        rt.lock().stop_hook_active = true;
        let message = CustomMessage::new("hooks-stop", reason)
            .display(false)
            .deliver_as(DeliverAs::FollowUp)
            .trigger_turn(true);
        host.send_message(message).await?;
    }
    Ok(EventReply::none())
}

/// `pi.events` channels for subagents, and whether each one starts or stops an agent.
pub const SUBAGENT_CHANNELS: &[(&str, bool)] = &[
    ("subagents:start", true),
    ("subagents:stop", false),
    ("subagent:async-started", true),
    ("subagent:async-complete", false),
    ("subagent:foreground-complete", false),
];

pub async fn watch_subagents(rt: Arc<Runtime>, host: Host) -> Result<()> {
    for (channel, start) in SUBAGENT_CHANNELS {
        let mut subscription = host.subscribe(channel).await?;
        let (rt, host) = (rt.clone(), host.clone());
        tokio::spawn(async move {
            while let Some(data) = subscription.recv().await {
                subagent_event(&rt, &host, *start, &data).await;
            }
        });
    }
    Ok(())
}

pub async fn subagent_event(rt: &Arc<Runtime>, host: &Host, start: bool, data: &Value) {
    let field = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| data.get(*k).and_then(Value::as_str))
            .map(str::to_owned)
    };
    let agent_id = field(&["agentId", "id", "runId"]).unwrap_or_default();
    let agent_type = field(&["agentType", "agent", "type"]).unwrap_or_default();
    let snap = host.snapshot().map(|s| (*s).clone()).unwrap_or_default();
    let mut payload = json!({ "agent_id": agent_id, "agent_type": agent_type });
    let event = if start {
        "SubagentStart"
    } else {
        "SubagentStop"
    };
    if !start {
        payload["stop_hook_active"] = json!(false);
        payload["last_assistant_message"] =
            json!(field(&["lastAssistantMessage", "result", "output"]));
    }
    let outcomes = dispatch(
        rt,
        host,
        &snap,
        Dispatch::new(event, Target::Value(agent_type), payload),
    )
    .await;
    let context = contexts(&outcomes);
    if start && !context.is_empty() {
        let data = json!({ "agentId": agent_id, "additionalContext": context.join("\n\n") });
        let _ = host.emit("hooks:subagent-context", data).await;
    }
    if let Some(reason) = first_block(&outcomes).filter(|_| !start) {
        let _ = host
            .emit(
                "hooks:subagent-stop-blocked",
                json!({ "agentId": agent_id, "reason": reason }),
            )
            .await;
    }
}
