use std::sync::Arc;

use pi_ext_sdk::event_types::{ToolCallEvent, ToolResultEvent};
use pi_ext_sdk::json::merge_patch_diff;
use pi_ext_sdk::prelude::*;

use crate::decide::{contexts, first_block, first_stop, fold_tool, stop_text};
use crate::dispatch::{Dispatch, dispatch};
use crate::matcher::Target;
use crate::output::Permission;
use crate::paths::env_file;
use crate::runtime::Runtime;
use crate::tools::{from_hook_input, hook_tool_name, to_hook_input};

fn tool_dispatch(event: &'static str, tool: &str, input: &Value, payload: Value) -> Dispatch {
    let mut d = Dispatch::new(event, Target::Tool(tool.to_owned()), payload);
    d.tool = Some((tool.to_owned(), input.clone()));
    d
}

pub async fn tool_call(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let call: ToolCallEvent = ev.event_as()?;
    let (host, snap) = (ev.host(), ev.snapshot());
    let tool = hook_tool_name(&call.tool_name);
    let mut input = to_hook_input(&call.tool_name, &call.input);
    let payload =
        json!({ "tool_name": tool, "tool_input": input, "tool_use_id": call.tool_call_id });
    let pre = fold_tool(
        &dispatch(
            &rt,
            host,
            snap,
            tool_dispatch("PreToolUse", &tool, &input, payload),
        )
        .await,
    );
    if let Some(reason) = pre.stop {
        return Ok(EventReply::block(stop_text(&reason)));
    }
    if !pre.context.is_empty() {
        rt.lock()
            .tool_context
            .entry(call.tool_call_id.clone())
            .or_default()
            .extend(pre.context);
    }
    let mut updated = false;
    if let Some(new_input) = pre.updated_input {
        input = new_input;
        updated = true;
    }
    let mut permission = pre.permission;
    if let Some((Permission::Deny, reason)) = permission {
        return Ok(EventReply::block(reason));
    }
    if !matches!(permission, Some((Permission::Allow, _))) {
        let payload = json!({ "tool_name": tool, "tool_input": input, "tool_use_id": call.tool_call_id, "permission_suggestions": [] });
        let request = fold_tool(
            &dispatch(
                &rt,
                host,
                snap,
                tool_dispatch("PermissionRequest", &tool, &input, payload),
            )
            .await,
        );
        if let Some(new_input) = request.updated_input {
            input = new_input;
            updated = true;
        }
        match request.permission {
            Some((Permission::Deny, reason)) => return Ok(EventReply::block(reason)),
            Some((Permission::Allow, reason)) => permission = Some((Permission::Allow, reason)),
            _ => {}
        }
    }
    if let Some((Permission::Ask, reason)) = &permission {
        let allowed = host.has_ui()
            && host
                .confirm(&format!("Hook: allow {tool}?"), reason)
                .await
                .unwrap_or(false);
        if !allowed {
            return Ok(EventReply::block(reason.clone()));
        }
    }
    let mut final_input = if updated {
        from_hook_input(&call.tool_name, &input)
    } else {
        call.input.clone()
    };
    if call.tool_name == "bash" {
        let session = snap.session_id.clone().unwrap_or_default();
        let file = env_file(&rt.agent_dir(host), &session);
        let has_env = std::fs::metadata(&file).is_ok_and(|m| m.len() > 0);
        if let (true, Some(command)) = (has_env, final_input.get("command").and_then(Value::as_str))
        {
            let quoted = file.display().to_string().replace('\'', r"'\''");
            final_input["command"] = json!(format!(". '{quoted}'; {command}"));
        }
    }
    Ok(match merge_patch_diff(&call.input, &final_input) {
        Some(diff) => EventReply::patch(json!({ "input": diff })),
        None => EventReply::none(),
    })
}

pub fn content_text(content: &[Value]) -> String {
    let texts: Vec<&str> = content
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect();
    texts.join("\n")
}

pub async fn tool_result(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let result: ToolResultEvent = ev.event_as()?;
    let (host, snap) = (ev.host(), ev.snapshot());
    let tool = hook_tool_name(&result.tool_name);
    let input = to_hook_input(&result.tool_name, &result.input);
    let text = content_text(&result.content);
    let mut payload = json!({
        "tool_name": tool, "tool_input": input, "tool_use_id": result.tool_call_id,
        "tool_response": { "content": result.content, "text": text, "isError": result.is_error, "details": result.details },
    });
    if result.is_error {
        payload["error"] = json!(text);
    }
    let mut d = tool_dispatch("PostToolUse", &tool, &input, payload);
    if result.is_error {
        d.claude = Some("PostToolUseFailure");
    }
    let outcomes = dispatch(&rt, host, snap, d).await;
    let held = rt
        .lock()
        .tool_context
        .remove(&result.tool_call_id)
        .unwrap_or_default();
    let mut content = result.content.clone();
    let mut appended: Vec<String> = held;
    for outcome in &outcomes {
        if let Some(output) = &outcome.updated_output {
            content = match output {
                Value::String(text) => vec![content::text(text.clone())],
                other => vec![content::text(other.to_string())],
            };
        }
    }
    for outcome in &outcomes {
        let codex = outcome.source.is_some_and(|s| s.is_codex());
        match &outcome.block {
            Some(reason) if codex => content = vec![content::text(reason.clone())],
            Some(reason) => appended.push(reason.clone()),
            None => {}
        }
        appended.extend(outcome.feedback.clone());
    }
    appended.extend(contexts(&outcomes));
    if let Some(reason) = first_stop(&outcomes) {
        appended.push(stop_text(&reason));
        let host = host.clone();
        tokio::spawn(async move {
            let _ = host.notify_warning(stop_text(&reason)).await;
            let _ = host.abort().await;
        });
    }
    content.extend(
        appended
            .into_iter()
            .filter(|t| !t.trim().is_empty())
            .map(content::text),
    );
    if content == result.content {
        return Ok(EventReply::none());
    }
    Ok(EventReply::patch(json!({ "content": content })))
}

pub async fn turn_end(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let results = ev
        .event()
        .get("toolResults")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if results.is_empty() {
        return Ok(EventReply::none());
    }
    let tool_results: Vec<Value> = results
        .iter()
        .map(|r| {
            let content: Vec<Value> = r.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
            json!({
                "tool_name": hook_tool_name(r.get("toolName").and_then(Value::as_str).unwrap_or_default()),
                "tool_use_id": r.get("toolCallId"),
                "is_error": r.get("isError").and_then(Value::as_bool).unwrap_or(false),
                "tool_response": content_text(&content),
            })
        })
        .collect();
    let d = Dispatch::new(
        "PostToolBatch",
        Target::Any,
        json!({ "tool_results": tool_results }),
    )
    .claude_only();
    let outcomes = dispatch(&rt, ev.host(), ev.snapshot(), d).await;
    if let Some(reason) = first_stop(&outcomes)
        .map(|r| stop_text(&r))
        .or_else(|| first_block(&outcomes))
    {
        let message = CustomMessage::new("hooks-feedback", reason).deliver_as(DeliverAs::Steer);
        ev.host().send_message(message).await?;
    }
    Ok(EventReply::none())
}
