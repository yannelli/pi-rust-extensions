//! Handlers for pi-rust-only host events. Older pi hosts never send them.
use std::sync::Arc;

use pi_ext_sdk::prelude::*;

use crate::decide::{contexts, first_block};
use crate::dispatch::{Dispatch, dispatch};
use crate::matcher::Target;
use crate::output::Outcome;
use crate::runtime::Runtime;
use crate::tools::{hook_tool_name, to_hook_input};

fn text(ev: &EventCtx, key: &str) -> String {
    ev.event()
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// The short name of an extension path such as `native:hooks` or `/x/ext/hooks/index.js`.
pub fn extension_name(path: &str) -> &str {
    let path = path.strip_prefix("native:").unwrap_or(path);
    let path = path.trim_end_matches(['/', '\\']);
    let last = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match last {
        "index.js" | "index.ts" | "package.json" => path.rsplit(['/', '\\']).nth(1).unwrap_or(last),
        _ => last,
    }
}

/// `instructions_loaded` -> InstructionsLoaded (`load_reason` is the matcher target).
pub async fn instructions_loaded(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let reason = match text(&ev, "reason").as_str() {
        "compact" => "compact",
        _ => "session_start",
    };
    let memory_type = match text(&ev, "scope").as_str() {
        "user" => "User",
        _ => "Project",
    };
    let payload = json!({ "file_path": text(&ev, "path"), "memory_type": memory_type, "load_reason": reason });
    let d =
        Dispatch::new("InstructionsLoaded", Target::Value(reason.into()), payload).claude_only();
    let (rt, host, snap) = (rt.clone(), ev.host().clone(), ev.snapshot().clone());
    tokio::spawn(async move {
        dispatch(&rt, &host, &snap, d).await;
    });
    Ok(EventReply::none())
}

/// `prompt_expansion` -> UserPromptExpansion: block, or add context to the expanded prompt.
pub async fn prompt_expansion(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let name = text(&ev, "name");
    let payload = json!({
        "expansion_type": "slash_command",
        "command_name": name,
        "command_args": text(&ev, "args"),
        "command_source": text(&ev, "kind"),
        "prompt": text(&ev, "prompt"),
    });
    let d =
        Dispatch::new("UserPromptExpansion", Target::Value(name.clone()), payload).claude_only();
    let outcomes = dispatch(&rt, ev.host(), ev.snapshot(), d).await;
    let stop = crate::decide::first_stop(&outcomes).map(|r| crate::decide::stop_text(&r));
    if let Some(reason) = stop.or_else(|| first_block(&outcomes)) {
        let _ = ev
            .host()
            .notify_warning(format!("/{name} blocked by hook: {reason}"))
            .await;
        return Ok(EventReply::result(
            json!({ "action": "block", "reason": reason }),
        ));
    }
    let context = contexts(&outcomes);
    if context.is_empty() {
        return Ok(EventReply::none());
    }
    Ok(EventReply::result(
        json!({ "action": "context", "text": context.join("\n\n") }),
    ))
}

/// `notification` -> Notification. Notices this extension raised are skipped.
pub async fn notification(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let extension = ev
        .event()
        .get("extension")
        .and_then(Value::as_str)
        .map(extension_name);
    if extension == Some("hooks") {
        return Ok(EventReply::none());
    }
    let kind = match text(&ev, "source").as_str() {
        "extension" => "plugin_notification",
        _ => "pi_notification",
    };
    let title = extension.unwrap_or("pi").to_owned();
    let message = text(&ev, "message");
    let (rt, host, snap) = (rt.clone(), ev.host().clone(), ev.snapshot().clone());
    tokio::spawn(async move {
        crate::events::watch::notify(&rt, &host, &snap, kind, &message, Some(&title)).await;
    });
    Ok(EventReply::none())
}

/// `tool_call_blocked` -> PermissionDenied for blocks by other extensions or pi's permissions.
pub async fn tool_call_blocked(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let id = text(&ev, "toolCallId");
    if ev.event().get("extension").and_then(Value::as_str) == Some("hooks")
        || rt.lock().own_blocks.remove(&id)
    {
        return Ok(EventReply::none());
    }
    let pi_tool = text(&ev, "toolName");
    let tool = hook_tool_name(&pi_tool);
    let input = ev.event().get("input").cloned().unwrap_or(json!({}));
    let input = to_hook_input(&pi_tool, &input, std::path::Path::new(&ev.snapshot().cwd));
    let reason = text(&ev, "reason");
    let payload =
        json!({ "tool_name": tool, "tool_input": input, "tool_use_id": id, "reason": reason });
    let mut d =
        Dispatch::new("PermissionDenied", Target::Tool(tool.clone()), payload).claude_only();
    d.tool = Some((tool.clone(), input));
    let (rt, host, snap) = (rt.clone(), ev.host().clone(), ev.snapshot().clone());
    tokio::spawn(async move {
        let outcomes = dispatch(&rt, &host, &snap, d).await;
        if outcomes.iter().any(|o| o.retry) {
            let note =
                format!("The {tool} call was denied ({reason}), but a hook says you may retry it.");
            let message = CustomMessage::new("hooks-retry", note)
                .display(false)
                .deliver_as(DeliverAs::Steer);
            let _ = host.send_message(message).await;
        }
    });
    Ok(EventReply::none())
}

/// `cwd_changed` -> CwdChanged.
pub async fn cwd_changed(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let old = text(&ev, "oldCwd");
    crate::events::watch::cwd_changed(&rt, ev.host(), ev.snapshot(), &old).await;
    Ok(EventReply::none())
}

/// `message_display` -> MessageDisplay; `displayContent` replaces the shown text.
pub async fn message_display(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let event = ev.event();
    let payload = json!({
        "turn_id": format!("turn-{}", rt.lock().turn),
        "message_id": event.get("messageId"),
        "index": event.get("index").cloned().unwrap_or(json!(0)),
        "final": event.get("final").cloned().unwrap_or(json!(true)),
        "delta": event.get("delta").or_else(|| event.get("text")),
    });
    let d = Dispatch::new("MessageDisplay", Target::Any, payload).claude_only();
    let outcomes = dispatch(&rt, ev.host(), ev.snapshot(), d).await;
    match outcomes
        .iter()
        .rev()
        .find_map(|o| o.display_content.clone())
    {
        Some(text) => Ok(EventReply::result(json!({ "text": text }))),
        None => Ok(EventReply::none()),
    }
}

/// Elicitation decisions: a decline (or block) wins over accept and cancel.
pub fn elicitation_answer(outcomes: &[Outcome]) -> Option<Value> {
    if first_block(outcomes).is_some() {
        return Some(json!({ "action": "decline" }));
    }
    let answers: Vec<&(String, Option<Value>)> = outcomes
        .iter()
        .filter_map(|o| o.elicitation.as_ref())
        .collect();
    if answers.iter().any(|(action, _)| action == "decline") {
        return Some(json!({ "action": "decline" }));
    }
    let (action, content) = answers.last()?;
    let mut answer = json!({ "action": action });
    if let Some(content) = content {
        answer["content"] = content.clone();
    }
    Some(answer)
}

/// `mcp_elicitation` -> Elicitation: a hook answer skips the dialog.
pub async fn mcp_elicitation(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let event = ev.event();
    let server = text(&ev, "server");
    let payload = json!({
        "mcp_server_name": server, "message": event.get("message"), "mode": event.get("mode"),
        "url": event.get("url"), "elicitation_id": event.get("elicitationId"),
        "requested_schema": event.get("requestedSchema"),
    });
    let d = Dispatch::new("Elicitation", Target::Value(server), payload).claude_only();
    let outcomes = dispatch(&rt, ev.host(), ev.snapshot(), d).await;
    Ok(match elicitation_answer(&outcomes) {
        Some(answer) => EventReply::result(answer),
        None => EventReply::none(),
    })
}

/// `mcp_elicitation_result` -> ElicitationResult: can change or block the user's answer.
pub async fn mcp_elicitation_result(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let event = ev.event();
    let server = text(&ev, "server");
    let payload = json!({
        "mcp_server_name": server, "action": event.get("action"), "mode": event.get("mode"),
        "elicitation_id": event.get("elicitationId"), "content": event.get("content"),
    });
    let d =
        Dispatch::new("ElicitationResult", Target::Value(server.clone()), payload).claude_only();
    let (host, snap) = (ev.host(), ev.snapshot());
    let outcomes = dispatch(&rt, host, snap, d).await;
    let message = format!("Sent an elicitation response to {server}");
    crate::events::watch::notify(&rt, host, snap, "elicitation_response", &message, None).await;
    Ok(match elicitation_answer(&outcomes) {
        Some(answer) => EventReply::result(answer),
        None => EventReply::none(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(action: &str, content: Option<Value>) -> Outcome {
        Outcome {
            elicitation: Some((action.into(), content)),
            ..Outcome::default()
        }
    }

    #[test]
    fn extension_names() {
        assert_eq!(extension_name("native:hooks"), "hooks");
        assert_eq!(extension_name("/a/ext/todo/index.js"), "todo");
        assert_eq!(extension_name("lens"), "lens");
    }

    #[test]
    fn decline_wins() {
        let accept = answer("accept", Some(json!({ "name": "a" })));
        assert_eq!(
            elicitation_answer(std::slice::from_ref(&accept)),
            Some(json!({ "action": "accept", "content": { "name": "a" } }))
        );
        let both = [accept.clone(), answer("decline", None)];
        assert_eq!(
            elicitation_answer(&both),
            Some(json!({ "action": "decline" }))
        );
        let blocked = Outcome {
            block: Some("no".into()),
            ..Outcome::default()
        };
        assert_eq!(
            elicitation_answer(&[accept, blocked]),
            Some(json!({ "action": "decline" }))
        );
        assert_eq!(elicitation_answer(&[Outcome::default()]), None);
    }
}
