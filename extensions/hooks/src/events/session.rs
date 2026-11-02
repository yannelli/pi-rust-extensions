use std::sync::Arc;

use pi_ext_sdk::event_types::{SessionShutdownEvent, SessionStartEvent};
use pi_ext_sdk::prelude::*;

use crate::decide::{contexts, first_block, first_stop, fold_tool, stop_text};
use crate::dispatch::{Dispatch, dispatch};
use crate::matcher::Target;
use crate::output::Permission;
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
    let (host, snap) = (ev.host(), ev.snapshot());
    let init_only = snap.flag_bool("init-only");
    if source == "startup" {
        setup(&rt, host, snap, init_only).await;
        crate::events::worktree::on_startup(&rt, host, snap).await;
    }
    run_session_start(&rt, host, snap, source).await;
    if init_only && source == "startup" {
        let host = host.clone();
        tokio::spawn(async move {
            if let Err(err) = host.pi_call("ctx", "shutdown", Vec::new()).await {
                pi_ext_sdk::warn!("hooks: --init-only cannot shut pi down: {err}");
            }
        });
    }
    Ok(EventReply::none())
}

/// Extension flags that start Claude Setup hooks, as `claude --init`, `--init-only`, `--maintenance`.
pub const SETUP_FLAGS: [(&str, &str); 3] = [
    ("init", "Run Setup hooks (trigger init) in print mode"),
    ("init-only", "Run Setup and SessionStart hooks, then exit"),
    (
        "maintenance",
        "Run Setup hooks (trigger maintenance) in print mode",
    ),
];

/// Setup fires for `--init-only`, or for `--init` / `--maintenance` without a UI.
async fn setup(rt: &Arc<Runtime>, host: &Host, snap: &Snapshot, init_only: bool) {
    let print = !snap.has_ui;
    let trigger = if init_only || (print && snap.flag_bool("init")) {
        "init"
    } else if print && snap.flag_bool("maintenance") {
        "maintenance"
    } else {
        return;
    };
    let d = Dispatch::new(
        "Setup",
        Target::Value(trigger.into()),
        json!({ "trigger": trigger }),
    )
    .claude_only();
    dispatch(rt, host, snap, d).await;
}

/// SessionStart: context is held for the next `before_agent_start`.
pub async fn run_session_start(rt: &Arc<Runtime>, host: &Host, snap: &Snapshot, source: &str) {
    let model = snap.model.as_ref().map(|m| m.id.clone());
    let mut payload = json!({ "source": source, "model": model });
    if matches!(source, "resume" | "fork")
        && let Some(cost) = resume_cost(host, snap).await
        && let (Value::Object(map), Value::Object(cost)) = (&mut payload, cost)
    {
        map.extend(cost);
    }
    let outcomes = dispatch(
        rt,
        host,
        snap,
        Dispatch::new("SessionStart", Target::Value(source.into()), payload),
    )
    .await;
    rt.lock().held_context.extend(contexts(&outcomes));
    crate::events::watch::apply_watch_paths(rt, &outcomes);
    for outcome in &outcomes {
        if let Some(title) = &outcome.session_title {
            let _ = host.set_session_name(title).await;
        }
        let print_mode = source == "startup" && !host.has_ui();
        if let Some(message) = outcome.initial_user_message.clone().filter(|_| print_mode) {
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
    if event.reason == "quit" {
        crate::events::worktree::on_quit(&rt, host, snap).await;
    }
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
    let restore = event.get("source").and_then(Value::as_str) == Some("restore");
    let source = match event.get("source").and_then(Value::as_str) {
        Some("set") => "command",
        Some("cycle") => "picker",
        Some("restore") => "resume",
        _ => "sdk",
    };
    let payload = json!({
        "from_model": previous.as_ref().map(|(_, id)| id), "to_model": id,
        "requested_model": if restore { json!(null) } else { json!(id) },
        "source": source,
    });
    let (host, snap) = (ev.host(), ev.snapshot());
    let mut payload = payload;
    let (rate, pricing) = cache_write_rate(host, &provider, &id).await;
    let tokens = context_tokens(snap);
    let warm = rt
        .lock()
        .last_response_at
        .is_some_and(|at| at.elapsed().as_secs() < CACHE_TTL_SECS);
    payload["context_tokens"] = json!(tokens);
    payload["prompt_cache_warm"] = json!(warm);
    payload["cache_ttl"] = json!("5m");
    payload["estimated_cache_write_usd"] = json!(usd(tokens, rate));
    payload["pricing"] = json!(pricing);
    if !restore && let Some(reason) = pre_model_switch(&rt, host, snap, &id, payload.clone()).await
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

/// The block reason, if PreModelSwitch hooks refuse the switch (deny > ask > allow).
async fn pre_model_switch(
    rt: &Arc<Runtime>,
    host: &Host,
    snap: &Snapshot,
    id: &str,
    payload: Value,
) -> Option<String> {
    let pre = Dispatch::new("PreModelSwitch", Target::Value(id.into()), payload).claude_only();
    let outcomes = dispatch(rt, host, snap, pre).await;
    if let Some(reason) = first_stop(&outcomes).map(|r| stop_text(&r)) {
        return Some(reason);
    }
    let timed_out = outcomes
        .iter()
        .find_map(|o| o.error.as_deref().filter(|e| e.starts_with("timed out")));
    if let Some(error) = timed_out {
        return Some(format!("PreModelSwitch hook {error}"));
    }
    match fold_tool(&outcomes).permission {
        Some((Permission::Deny, reason)) => Some(reason),
        Some((Permission::Ask, reason)) => {
            let title = format!("Hook: switch to {id}?");
            let allowed = host.has_ui()
                && crate::events::tool::confirm_with_notice(rt, host, snap, &title, &reason).await;
            (!allowed).then_some(reason)
        }
        _ => None,
    }
}

const CACHE_TTL_SECS: u64 = 300;
/// USD per million cache-write tokens when the model has no known price.
const DEFAULT_CACHE_WRITE: f64 = 3.75;

fn context_tokens(snap: &Snapshot) -> u64 {
    snap.context_usage
        .as_ref()
        .and_then(|u| u.get("tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn usd(tokens: u64, rate: f64) -> f64 {
    (tokens as f64 * rate / 1_000_000.0 * 10_000.0).round() / 10_000.0
}

/// The model's `cost.cacheWrite` (USD per million tokens) and `"catalog"`, else the default rate.
async fn cache_write_rate(host: &Host, provider: &str, id: &str) -> (f64, &'static str) {
    let model = host.find_model(provider, id).await.ok().flatten();
    let rate = model
        .as_ref()
        .and_then(|m| m.extra.get("cost"))
        .and_then(|c| c.get("cacheWrite"))
        .and_then(Value::as_f64)
        .filter(|r| *r > 0.0);
    match rate {
        Some(rate) => (rate, "catalog"),
        None => (DEFAULT_CACHE_WRITE, "default"),
    }
}

/// SessionStart resume fields, when the transcript has an assistant response.
async fn resume_cost(host: &Host, snap: &Snapshot) -> Option<Value> {
    let branch = host.branch(None).await.ok()?;
    let last = branch
        .iter()
        .rev()
        .filter_map(|e| e.get("message"))
        .find(|m| m.get("role").and_then(Value::as_str) == Some("assistant"))?;
    let at = last.get("timestamp").and_then(Value::as_u64)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let seconds = now.saturating_sub(at) / 1000;
    let tokens = context_tokens(snap);
    let (rate, _) = match &snap.model {
        Some(m) => cache_write_rate(host, &m.provider, &m.id).await,
        None => (DEFAULT_CACHE_WRITE, "default"),
    };
    Some(json!({
        "seconds_since_last_response": seconds,
        "context_tokens": tokens,
        "prompt_cache_likely_expired": seconds >= CACHE_TTL_SECS,
        "estimated_cache_write_usd": usd(tokens, rate),
    }))
}
