//! The `deferred_search` tool: finds deferred items and loads them.

use pi_ext_sdk::prelude::*;
use serde::Deserialize;

use crate::catalog::{Body, Item, TOOL};
use crate::config::Kind;
use crate::{LOAD_ENTRY, Shared, fmt, lock, search};

#[derive(Deserialize)]
pub(crate) struct Args {
    #[serde(default)]
    query: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    max_results: Option<usize>,
}

pub(crate) fn spec() -> ToolSpec {
    let description = "Search for and load deferred tools, skills, project instructions, and prompt \
        sections. A loaded tool is callable from your next call; skills, instructions, and sections \
        come back as text in the result.";
    ToolSpec::new(TOOL, description)
        .label("Deferred search")
        .prompt_snippet("Search and load deferred tools, skills, and instructions")
        .parameters(json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "`select:<name>[,<name>]` loads exact names. Otherwise keywords; prefix a word with + to require it. An empty query lists deferred items."
                },
                "kind": {
                    "type": "string",
                    "enum": ["any", "tool", "skill", "context", "section"],
                    "description": "Only search this kind of item. Default any."
                },
                "max_results": {
                    "type": "integer", "minimum": 1, "maximum": 50,
                    "description": "Most items to load for a keyword search. Default 5."
                }
            },
            "required": ["query"]
        }))
}

pub(crate) async fn run(state: Shared, args: Args, ctx: ToolCtx) -> Result<ToolResult> {
    let kind = match args.kind.as_deref().map(str::trim) {
        None | Some("" | "any") => None,
        Some(text) => match Kind::parse(text) {
            Some(kind) => Some(kind),
            None => return Ok(ToolResult::error(format!("Unknown kind `{text}`"))),
        },
    };
    let (items, default_max, loaded) = {
        let session = lock(&state);
        let items = session
            .plan
            .as_ref()
            .map(|p| p.items.clone())
            .unwrap_or_default();
        (items, session.config.max_results, session.loaded.clone())
    };
    let max = args.max_results.unwrap_or(default_max).clamp(1, 50);
    let query = search::parse(&args.query);
    let found: Vec<Item> = search::find(&items, &query, kind, max)
        .into_iter()
        .cloned()
        .collect();
    let already: Vec<&String> = query
        .select
        .iter()
        .filter(|n| loaded.contains(*n))
        .collect();
    if found.is_empty() {
        let text = match already.is_empty() {
            true => fmt::no_match(&args.query, &items),
            false => format!("Already loaded: {}.", join(&already)),
        };
        return Ok(ToolResult::text(text).details(json!({ "loaded": [] })));
    }
    let tools: Vec<String> = found
        .iter()
        .filter(|i| i.kind == Kind::Tool)
        .map(|i| i.name.clone())
        .collect();
    if !tools.is_empty() {
        if let Err(err) = activate(&state, &ctx, &tools).await {
            return Ok(ToolResult::error(format!(
                "Could not load tools: {}",
                err.message()
            )));
        }
    }
    let mut parts = Vec::new();
    if !tools.is_empty() {
        let lines: Vec<String> = found
            .iter()
            .filter(|i| i.kind == Kind::Tool)
            .map(|i| fmt::loaded(i, None))
            .collect();
        parts.push(format!(
            "Loaded {} tool(s). They are callable from your next call:\n{}",
            tools.len(),
            lines.join("\n")
        ));
    }
    for item in found.iter().filter(|i| i.kind != Kind::Tool) {
        let text = match &item.body {
            Body::Skill { file, .. } => tokio::fs::read_to_string(file).await.ok(),
            _ => None,
        };
        parts.push(fmt::loaded(item, text.as_deref()));
    }
    if !already.is_empty() {
        parts.push(format!("Already loaded: {}.", join(&already)));
    }
    let names: Vec<&String> = found.iter().map(|i| &i.name).collect();
    Ok(ToolResult::text(parts.join("\n\n")).details(json!({ "loaded": names, "tools": tools })))
}

fn join(names: &[&String]) -> String {
    names
        .iter()
        .map(|n| n.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Adds `tools` to the loadout and records them for this session.
async fn activate(state: &Shared, ctx: &ToolCtx, tools: &[String]) -> Result<()> {
    let host = ctx.host();
    let mut active = host.active_tools().await?;
    for name in tools {
        if !active.contains(name) {
            active.push(name.clone());
        }
    }
    host.set_active_tools(&active).await?;
    {
        let mut session = lock(state);
        for name in tools {
            session.removed.remove(name);
            session.loaded.insert(name.clone());
        }
        if let Some(plan) = session.plan.as_mut() {
            plan.items
                .retain(|i| i.kind != Kind::Tool || !tools.contains(&i.name));
        }
    }
    if let Err(err) = host
        .append_entry(LOAD_ENTRY, &json!({ "tools": tools }))
        .await
    {
        pi_ext_sdk::warn!("appendEntry {LOAD_ENTRY} failed: {}", err.message());
    }
    Ok(())
}
