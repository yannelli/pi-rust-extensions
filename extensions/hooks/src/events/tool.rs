use std::path::Path;
use std::sync::Arc;

use pi_ext_sdk::event_types::{ToolCallEvent, ToolResultEvent};
use pi_ext_sdk::json::merge_patch_diff;
use pi_ext_sdk::prelude::*;

use crate::decide::{contexts, first_block, first_stop, fold_tool, stop_text};
use crate::dispatch::{Dispatch, dispatch};
use crate::events::tasks::{TODO_TOOL, todo_call};
use crate::matcher::Target;
use crate::model::Source;
use crate::output::{Outcome, Permission};
use crate::paths::env_file;
use crate::runtime::Runtime;
use crate::tools::{
    codex_tool_input, codex_tool_name, from_hook_input, hook_tool_name, to_hook_input,
};

fn tool_dispatch(event: &'static str, tool: &str, input: &Value, payload: Value) -> Dispatch {
    let mut d = Dispatch::new(event, Target::Tool(tool.to_owned()), payload);
    d.tool = Some((tool.to_owned(), input.clone()));
    d
}

/// Codex `tool_name` and `tool_input` (`apply_patch` for edits), merged over the Claude payload.
fn codex_fields(pi_tool: &str, tool: &str, input: &Value) -> Value {
    json!({ "tool_name": codex_tool_name(tool), "tool_input": codex_tool_input(pi_tool, input) })
}

/// Codex `updatedInput` for `apply_patch` holds patch text that pi edit and write cannot take.
fn drop_patch_rewrites(pi_tool: &str, outcomes: &mut [Outcome]) {
    if !matches!(pi_tool, "edit" | "write") {
        return;
    }
    for outcome in outcomes
        .iter_mut()
        .filter(|o| o.source == Some(Source::Codex))
    {
        if outcome.updated_input.take().is_some() {
            pi_ext_sdk::warn!("hooks: Codex updatedInput for apply_patch is not supported");
        }
    }
}

fn stop_run(host: &Host, reason: String) {
    let host = host.clone();
    tokio::spawn(async move {
        let _ = host.notify_warning(reason).await;
        let _ = host.abort().await;
    });
}

pub async fn tool_call(rt: Arc<Runtime>, ev: EventCtx) -> Result<EventReply> {
    let call: ToolCallEvent = ev.event_as()?;
    let (host, snap) = (ev.host(), ev.snapshot());
    rt.lock().stop_blocks = 0;
    if call.tool_name.starts_with("mcp__") {
        load_mcp_sources(&rt, host).await;
    }
    let tool = hook_tool_name(&call.tool_name);
    let mut input = to_hook_input(&call.tool_name, &call.input, Path::new(&snap.cwd));
    let payload =
        json!({ "tool_name": tool, "tool_input": input, "tool_use_id": call.tool_call_id });
    let mut d = tool_dispatch("PreToolUse", &tool, &input, payload);
    d.codex_payload = Some(codex_fields(&call.tool_name, &tool, &input));
    let mut outcomes = dispatch(&rt, host, snap, d).await;
    drop_patch_rewrites(&call.tool_name, &mut outcomes);
    let pre = fold_tool(&outcomes);
    if let Some(reason) = pre.stop {
        let reason = stop_text(&reason);
        stop_run(host, reason.clone());
        return Ok(own_block(&rt, &call.tool_call_id, reason));
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
        return Ok(own_block(&rt, &call.tool_call_id, reason));
    }
    if !matches!(permission, Some((Permission::Allow, _))) {
        let payload = json!({ "tool_name": tool, "tool_input": input, "tool_use_id": call.tool_call_id, "permission_suggestions": [] });
        let mut d = tool_dispatch("PermissionRequest", &tool, &input, payload);
        d.codex_payload = Some(codex_fields(&call.tool_name, &tool, &input));
        let request = fold_tool(&dispatch(&rt, host, snap, d).await);
        if let Some(new_input) = request.updated_input {
            input = new_input;
            updated = true;
        }
        if request.interrupt {
            stop_run(host, "A PermissionRequest hook stopped the run".into());
        }
        match request.permission {
            Some((Permission::Deny, reason)) => {
                return Ok(own_block(&rt, &call.tool_call_id, reason));
            }
            Some((Permission::Allow, reason)) => permission = Some((Permission::Allow, reason)),
            _ => {}
        }
    }
    if let Some((Permission::Ask, reason)) = &permission {
        let allowed = host.has_ui()
            && confirm_with_notice(&rt, host, snap, &format!("Hook: allow {tool}?"), reason).await;
        if !allowed {
            return Ok(own_block(&rt, &call.tool_call_id, reason.clone()));
        }
    }
    if call.tool_name == TODO_TOOL {
        let todo_input = if updated {
            from_hook_input(&call.tool_name, &input)
        } else {
            call.input.clone()
        };
        let (block, context) = todo_call(&rt, host, snap, &todo_input).await;
        if let Some(reason) = block {
            return Ok(own_block(&rt, &call.tool_call_id, reason));
        }
        if !context.is_empty() {
            rt.lock()
                .tool_context
                .entry(call.tool_call_id.clone())
                .or_default()
                .extend(context);
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
    rt.lock()
        .tool_started
        .insert(call.tool_call_id.clone(), std::time::Instant::now());
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
    let input = to_hook_input(&result.tool_name, &result.input, Path::new(&snap.cwd));
    let text = content_text(&result.content);
    let mut codex = codex_fields(&result.tool_name, &tool, &input);
    codex["tool_response"] = json!(text);
    let mut payload = json!({
        "tool_name": tool, "tool_input": input, "tool_use_id": result.tool_call_id,
        "tool_response": { "content": result.content, "text": text, "isError": result.is_error, "details": result.details },
    });
    if result.is_error {
        payload["error"] = json!(text);
        payload["is_interrupt"] = json!(text.trim() == "Operation aborted");
    }
    if let Some(started) = rt.lock().tool_started.remove(&result.tool_call_id) {
        payload["duration_ms"] = json!(started.elapsed().as_millis() as u64);
    }
    let mut d = tool_dispatch("PostToolUse", &tool, &input, payload);
    d.codex_payload = Some(codex);
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
    for outcome in &outcomes {
        let Some(reason) = outcome.stop.as_deref().map(stop_text) else {
            continue;
        };
        if outcome.source == Some(Source::Codex) {
            content = vec![content::text(reason)];
        } else {
            appended.push(reason.clone());
            stop_run(host, reason);
            break;
        }
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
    let cwd = Path::new(&ev.snapshot().cwd).to_path_buf();
    let tool_calls: Vec<Value> = results
        .iter()
        .map(|r| {
            let content: Vec<Value> = r
                .get("content")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let pi_tool = r
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let input = call_arguments(ev.event(), r.get("toolCallId"));
            json!({
                "tool_name": hook_tool_name(pi_tool),
                "tool_input": to_hook_input(pi_tool, &input, &cwd),
                "tool_use_id": r.get("toolCallId"),
                "tool_response": content_text(&content),
            })
        })
        .collect();
    let d = Dispatch::new(
        "PostToolBatch",
        Target::Any,
        json!({ "tool_calls": tool_calls }),
    )
    .claude_only();
    let host = ev.host();
    let outcomes = dispatch(&rt, host, ev.snapshot(), d).await;
    let context = contexts(&outcomes);
    if !context.is_empty() {
        let message = CustomMessage::new("hooks-context", context.join("\n\n"))
            .display(false)
            .deliver_as(DeliverAs::Steer);
        host.send_message(message).await?;
    }
    if let Some(reason) = first_stop(&outcomes)
        .map(|r| stop_text(&r))
        .or_else(|| first_block(&outcomes))
    {
        let message =
            CustomMessage::new("hooks-feedback", reason.clone()).deliver_as(DeliverAs::NextTurn);
        host.send_message(message).await?;
        stop_run(host, reason);
    }
    Ok(EventReply::none())
}

/// The arguments of a tool call in the `turn_end` assistant message.
fn call_arguments(event: &Value, id: Option<&Value>) -> Value {
    event
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|block| {
            block.get("type").and_then(Value::as_str) == Some("toolCall") && block.get("id") == id
        })
        .and_then(|block| block.get("arguments").cloned())
        .unwrap_or_else(|| json!({}))
}

/// `ui.confirm` that fires Notification `permission_prompt` when it stays open for 6 seconds.
pub async fn confirm_with_notice(
    rt: &Arc<Runtime>,
    host: &Host,
    snap: &Snapshot,
    title: &str,
    message: &str,
) -> bool {
    let notice = {
        let (rt, host, snap) = (rt.clone(), host.clone(), snap.clone());
        let text = format!("{title} {message}");
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(6)).await;
            crate::events::watch::notify(&rt, &host, &snap, "permission_prompt", &text, None).await;
        })
    };
    let allowed = host.confirm(title, message).await.unwrap_or(false);
    notice.abort();
    allowed
}

/// Blocks a tool call and remembers it, so `tool_call_blocked` skips this extension's own blocks.
fn own_block(rt: &Runtime, id: &str, reason: impl Into<String>) -> EventReply {
    rt.lock().own_blocks.insert(id.to_owned());
    EventReply::block(reason)
}

/// Fills the MCP server source cache once per session from `pi.getMcpServers()`.
async fn load_mcp_sources(rt: &Runtime, host: &Host) {
    if rt.lock().mcp_sources.is_some() {
        return;
    }
    let servers = host
        .pi_call("pi", "getMcpServers", Vec::new())
        .await
        .unwrap_or(Value::Null);
    let sources = servers
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let name = s.get("name")?.as_str()?.to_owned();
            let source = ["source", "scope"]
                .iter()
                .find_map(|k| s.get(*k).and_then(Value::as_str))
                .unwrap_or("user");
            Some((name, source.to_owned()))
        })
        .collect();
    rt.lock().mcp_sources = Some(sources);
}
