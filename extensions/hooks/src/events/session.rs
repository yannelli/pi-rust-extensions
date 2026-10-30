use std::sync::Arc;

use pi_ext_sdk::event_types::{SessionShutdownEvent, SessionStartEvent};
use pi_ext_sdk::prelude::*;

use crate::decide::{contexts, first_block, first_stop, stop_text};
use crate::dispatch::{Dispatch, dispatch};
use crate::matcher::Target;
use crate::runtime::Runtime;

pub async fn session_start(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let event: SessionStartEvent = ev.event_as()?;
    rt.reset_session();
    rt.reload(ev.host(), ev.snapshot()).await;
    let source = match event.reason.as_str() {
        "reload" | "resume" => "resume",
        "new" => "clear",
        "fork" => "fork",
        _ => "startup",
    };
    run_session_start(&rt, ev.host(), ev.snapshot(), source).await;
    Ok(EventReply::none())
}

/// SessionStart: context is held for the next `before_agent_start`.
pub async fn run_session_start(rt: &Arc<Runtime>, host: &Host, snap: &Snapshot, source: &str) {
    let model = snap.model.as_ref().map(|m| m.id.clone());
    let payload = json!({ "source": source, "model": model });
    let outcomes = dispatch(
        rt,
        host,
        snap,
        Dispatch::new("SessionStart", Target::Value(source.into()), payload),
    )
    .await;
    rt.lock().held_context.extend(contexts(&outcomes));
    for outcome in &outcomes {
        if let Some(title) = &outcome.session_title {
            let _ = host.set_session_name(title).await;
        }
        if let Some(message) = outcome.initial_user_message.clone() {
            let host = host.clone();
            tokio::spawn(async move {
                if let Err(err) = host.send_user_message(message, None).await {
                    pi_ext_sdk::warn!("hooks: initialUserMessage failed: {err}");
                }
            });
        }
    }
}

pub async fn session_shutdown(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let event: SessionShutdownEvent = ev.event_as()?;
    let reason = match event.reason.as_str() {
        "quit" => "prompt_input_exit",
        "new" => "clear",
        "resume" => "resume",
        _ => "other",
    };
    let (host, snap) = (ev.host(), ev.snapshot());
    let claude = Dispatch::new(
        "SessionEnd",
        Target::Value(reason.into()),
        json!({ "reason": reason }),
    )
    .claude_only();
    let mut codex = Dispatch::new(
        "SessionEnd",
        Target::Value("other".into()),
        json!({ "reason": "other" }),
    );
    codex.claude = None;
    tokio::join!(
        dispatch(&rt, host, snap, claude),
        dispatch(&rt, host, snap, codex)
    );
    Ok(EventReply::none())
}

pub async fn before_agent_start(rt: Arc<Runtime>, _ev: EventCtx) -> Result<EventReply> {
    let held = std::mem::take(&mut rt.lock().held_context);
    if held.is_empty() {
        return Ok(EventReply::none());
    }
    let message =
        json!({ "customType": "hooks-context", "content": held.join("\n\n"), "display": false });
    Ok(EventReply::result(json!({ "message": message })))
}

fn trigger(ev: &EventCtx) -> &'static str {
    match ev.event().get("reason").and_then(Value::as_str) {
        Some("manual") => "manual",
        _ => "auto",
    }
}

pub async fn session_before_compact(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let trigger = trigger(&ev);
    let payload =
        json!({ "trigger": trigger, "custom_instructions": ev.event().get("customInstructions") });
    let d = Dispatch::new("PreCompact", Target::Value(trigger.into()), payload);
    let outcomes = dispatch(&rt, ev.host(), ev.snapshot(), d).await;
    match first_stop(&outcomes)
        .map(|r| stop_text(&r))
        .or_else(|| first_block(&outcomes))
    {
        Some(reason) => {
            let _ = ev
                .host()
                .notify_warning(format!("Compaction cancelled by hook: {reason}"))
                .await;
            Ok(EventReply::cancel())
        }
        None => Ok(EventReply::none()),
    }
}

pub async fn session_compact(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let trigger = trigger(&ev);
    let summary = ev.event().pointer("/compactionEntry/summary").cloned();
    let payload = json!({ "trigger": trigger, "compact_summary": summary });
    let (host, snap) = (ev.host(), ev.snapshot());
    dispatch(
        &rt,
        host,
        snap,
        Dispatch::new("PostCompact", Target::Value(trigger.into()), payload),
    )
    .await;
    run_session_start(&rt, host, snap, "compact").await;
    Ok(EventReply::none())
}

fn model_ref(model: &Value) -> Option<(String, String)> {
    let provider = model.get("provider")?.as_str()?.to_owned();
    let id = model.get("id")?.as_str()?.to_owned();
    Some((provider, id))
}

/// pi fires `model_select` after the switch, so a PreModelSwitch block switches back.
pub async fn model_select(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let event = ev.event();
    let Some((provider, id)) = event.get("model").and_then(model_ref) else {
        return Ok(EventReply::none());
    };
    let previous = event.get("previousModel").and_then(model_ref);
    let reverting = rt.lock().reverting_model.take();
    if reverting.as_deref() == Some(&format!("{provider}/{id}")) {
        return Ok(EventReply::none());
    }
    let payload = json!({
        "model": id, "provider": provider,
        "previous_model": previous.as_ref().map(|(_, id)| id), "previous_provider": previous.as_ref().map(|(p, _)| p),
        "source": event.get("source"),
    });
    let (host, snap) = (ev.host(), ev.snapshot());
    let pre =
        Dispatch::new("PreModelSwitch", Target::Value(id.clone()), payload.clone()).claude_only();
    let outcomes = dispatch(&rt, host, snap, pre).await;
    if let Some(reason) = first_stop(&outcomes)
        .map(|r| stop_text(&r))
        .or_else(|| first_block(&outcomes))
    {
        let _ = host
            .notify_warning(format!("Model switch blocked by hook: {reason}"))
            .await;
        if let Some((provider, id)) = previous {
            rt.lock().reverting_model = Some(format!("{provider}/{id}"));
            let host = host.clone();
            tokio::spawn(async move {
                if !host.set_model(&provider, &id).await.unwrap_or(false) {
                    pi_ext_sdk::warn!("hooks: cannot switch back to {provider}/{id}");
                }
            });
        }
        return Ok(EventReply::none());
    }
    let post = Dispatch::new("PostModelSwitch", Target::Value(id), payload).claude_only();
    let outcomes = dispatch(&rt, host, snap, post).await;
    rt.lock().held_context.extend(contexts(&outcomes));
    Ok(EventReply::none())
}
