//! Slash commands, the two tools, and compact-then autocomplete.

use std::path::PathBuf;

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::{AutocompleteRequest, CompletionItem, Suggestions};

use crate::config::Method;
use crate::doctor::{HELP, context_budget_text, doctor};
use crate::recall::{RecallInput, parse_command_args, scope_is_all, vcc_recall};
use crate::source::COMPACTOR_INSTRUCTION;
use crate::state::{COMPACT_THEN, Shared, load_config, spawn_compact};
use crate::stats::stats_text;
use crate::text::recall_blocks;

pub fn run_compaction(
    ctx: &CommandCtx,
    shared: &Shared,
    method: Method,
    args: &str,
    command: &str,
) {
    let trimmed = args.trim();
    shared.lock().set_pending(Some(method), true, Some(command));
    let instructions = match method {
        Method::Llm => Some(trimmed.to_owned()).filter(|t| !t.is_empty()),
        Method::Vcc if trimmed.is_empty() => Some(COMPACTOR_INSTRUCTION.to_owned()),
        Method::Vcc => Some(format!("{COMPACTOR_INSTRUCTION} {trimmed}")),
    };
    spawn_compact(ctx.host().clone(), shared.clone(), instructions, None, true);
}

async fn recall_entries(host: &Host, all: bool) -> Result<Vec<Value>> {
    if all {
        host.entries().await
    } else {
        host.branch(None).await
    }
}

pub async fn session_recall_command(args: String, ctx: CommandCtx) -> Result<()> {
    let host = ctx.host();
    let usage = "Usage: /unipi:session-recall <query>";
    let (query, all, page) = parse_command_args(args.trim());
    if query.is_empty() {
        return host.notify_warning(usage).await;
    }
    let blocks = recall_blocks(&recall_entries(host, all).await?);
    if blocks.is_empty() {
        return host
            .notify_warning("No session history available for search.")
            .await;
    }
    let input = RecallInput {
        query: Some(query),
        scope: all.then(|| "all".to_owned()),
        page: Some(page as f64),
        ..RecallInput::default()
    };
    let text = vcc_recall(&blocks, &input, &ctx.snapshot().cwd);
    let details = json!({ "unipiHarness": { "source": "Compactor", "title": "Session recall", "synopsis": "Results from /unipi:session-recall" } });
    let mut message = CustomMessage::new("compactor-recall", text)
        .display(true)
        .details(details)
        .trigger_turn(true);
    if ctx.snapshot().is_idle == Some(false) {
        message = message.deliver_as(DeliverAs::FollowUp);
    }
    host.send_message(message).await
}

pub async fn stats_command(ctx: CommandCtx) -> Result<()> {
    let branch = ctx.host().branch(None).await?;
    ctx.host()
        .notify_info(stats_text(&branch, ctx.snapshot().context_usage.as_ref()))
        .await
}

fn legacy_db() -> Option<(String, f64)> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let dir = home.join(".unipi").join("db").join("compactor");
    let entries = std::fs::read_dir(&dir).ok()?;
    let bytes: u64 = entries
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum();
    Some((dir.display().to_string(), bytes as f64 / (1024.0 * 1024.0)))
}

pub async fn doctor_command(ctx: CommandCtx) -> Result<()> {
    let host = ctx.host();
    let config = load_config(host, ctx.snapshot());
    let pi_auto = match host.all_settings().await {
        Ok(settings) => Some(settings["compaction"]["enabled"].as_bool().unwrap_or(true)),
        Err(_) => None,
    };
    let (healthy, text) = doctor(
        &config,
        ctx.snapshot().model.is_some(),
        pi_auto,
        legacy_db(),
    );
    if healthy {
        host.notify_info(text).await
    } else {
        host.notify_warning(text).await
    }
}

pub async fn help_command(ctx: CommandCtx) -> Result<()> {
    ctx.host().notify_info(HELP).await
}

pub fn recall_spec() -> ToolSpec {
    let description = "Search session history using keyword or regex search. Find previous goals, files, commits, decisions, and context — \
including anything dropped by compaction. Reach for this before telling the user you no longer have the context. \
Plain keywords work best; a regex pattern is also accepted. Results are paged (page); pass expand with entry indices \
to read full untruncated content. Use mode:'touched' to list files worked on in this session with their entry indices, \
and #N:path to drill into a file's content from an entry (#N:path:full for all lines). Only the current session is \
searchable — earlier sessions are not.";
    ToolSpec::new("session_recall", description).label("Session Recall").parameters(json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "What to recall, in plain keywords (e.g. 'redis cache decision'). Multi-word queries are ranked by relevance. A regex pattern also works. #N:path drills into a file's content from an entry." },
            "expand": { "type": "array", "items": { "type": "number" }, "description": "Entry indices to return full untruncated content for" },
            "page": { "type": "number", "minimum": 1, "description": "Page number (1-based) for paginated search results. Default: 1." },
            "scope": { "type": "string", "enum": ["lineage", "all"], "description": "Default 'lineage' covers the active conversation path. Use 'all' to also reach messages from other branches, such as turns that were edited or retried." },
            "mode": { "type": "string", "enum": ["hybrid", "touched"], "description": "What to show. hybrid (default) = normal search; touched = aggregated files-by-path with entry indices." }
        }
    }))
}

pub async fn recall_tool(input: RecallInput, ctx: ToolCtx, shared: Shared) -> Result<ToolResult> {
    shared.lock().recall_queries += 1;
    let entries = recall_entries(ctx.host(), scope_is_all(input.scope.as_deref())).await?;
    let text = vcc_recall(&recall_blocks(&entries), &input, &ctx.snapshot().cwd);
    Ok(ToolResult::text(text).details(json!({ "query": input.query })))
}

pub fn budget_spec() -> ToolSpec {
    let description = "Estimate how full the context window is (% full, tokens left). Compaction is automatic; this never needs to be acted on.";
    ToolSpec::new("context_budget", description).label("Context Budget")
}

pub async fn budget_tool(ctx: ToolCtx) -> Result<ToolResult> {
    let host = ctx.host();
    let config = load_config(host, ctx.snapshot());
    let usage = match ctx.snapshot().context_usage.clone() {
        Some(usage) if !usage.is_null() => Some(usage),
        _ => host.context_usage().await.ok().flatten(),
    };
    Ok(ToolResult::text(context_budget_text(
        usage.as_ref(),
        &config,
    )))
}

/// Suggests `/unipi:compact-then`, which is caught in `input` and is no registered command.
pub async fn compact_then_suggestions(request: AutocompleteRequest) -> Result<Option<Suggestions>> {
    let before = request.before_cursor();
    let Some(query) = before.strip_prefix('/').filter(|q| !q.contains(' ')) else {
        return Ok(None);
    };
    let query = query.to_lowercase();
    let long_enough = query.len() >= "unipi:compact-t".len()
        || (query.starts_with("compact-t") && !query.starts_with("unipi"));
    let matches = COMPACT_THEN.starts_with(&query) || "compact-then".starts_with(&query);
    if !long_enough || !matches {
        return Ok(None);
    }
    let item = CompletionItem::new(COMPACT_THEN)
        .label(COMPACT_THEN)
        .description("Compacts, then sends a prompt once it lands (not a registered command)");
    Ok(Some(Suggestions::new(vec![item], before)))
}
