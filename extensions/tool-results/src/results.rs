//! The `tool_result` handler, `tool_result_get`, and the expanded view of a stored result.

use pi_ext_sdk::ToolResultRender;
use pi_ext_sdk::event_types::ToolResultEvent;
use pi_ext_sdk::prelude::*;

use crate::expand::{self, DETAILS_KEY};
use crate::lines::{group, short};
use crate::pipeline::{self, GET_TOOL};
use crate::store::{self, Meta, Store};
use crate::{Shared, lock, retrieve, stats};

pub(crate) fn store_of(host: &Host) -> Store {
    Store::new(store::root(&host.agent_dir()))
}

fn texts(content: &[Value]) -> Vec<&str> {
    let text = content.iter().filter(|b| b["type"] == "text");
    text.map(|b| b["text"].as_str().unwrap_or_default())
        .collect()
}

pub(crate) async fn tool_result(state: Shared, ev: EventCtx) -> Result<EventReply> {
    let event: ToolResultEvent = ev.event_as()?;
    if event.tool_name == GET_TOOL {
        return Ok(EventReply::none());
    }
    let config = lock(&state).config.clone();
    let settings = config.for_tool(&event.tool_name);
    let settings = match event.is_error {
        true => settings.for_error(),
        false => Some(settings),
    };
    let Some(settings) = settings.filter(|s| s.enabled) else {
        return Ok(EventReply::none());
    };
    let raw = texts(&event.content).join("\n");
    let size = raw.chars().count();
    let id = store::result_id(&event.tool_call_id, &store::hash(&raw));
    let store = store_of(ev.host());
    let snapshot = ev.snapshot().clone();
    let (content, dry) = (event.content.clone(), config.dry_run);
    let (tool, call) = (event.tool_name.clone(), event.tool_call_id.clone());
    let worker = tokio::task::spawn_blocking(move || {
        let outcome = pipeline::process(&content, &settings, &id, config.context_chars)?;
        let must = outcome.truncated || outcome.stored.is_some();
        if dry || !(must || config.expand_original || config.embed_original) {
            return Some((outcome, None));
        }
        let meta = Meta {
            id: id.clone(),
            tool,
            tool_call_id: call,
            session_id: snapshot.session_id.clone(),
            session_file: snapshot.session_file.clone(),
            chars: outcome.full.chars().count(),
            lines: outcome.full.lines().count(),
            hash: store::hash(&outcome.full),
            created: store::now_secs(),
            bytes: 0,
        };
        if let Err(err) = store.save(&meta, &outcome.full) {
            pi_ext_sdk::warn!("tool-results: cannot store {id}: {err}");
            match (must, config.embed_original) {
                (true, false) => return None,
                (false, false) => return Some((outcome, None)),
                _ => {}
            }
        }
        let record = expand::record(&meta, &outcome.full, config.embed_original);
        Some((outcome, Some(record)))
    });
    let outcome = worker
        .await
        .map_err(|err| Error::msg(format!("tool-results worker failed: {err}")))?;
    let agent_dir = ev.host().agent_dir();
    let mut session = lock(&state);
    let changed = outcome.as_ref().map(|(o, _)| o);
    session.stats.record(&event.tool_name, changed, size, dry);
    let _ = session.stats.save(&stats::path(&agent_dir));
    drop(session);
    let Some((outcome, record)) = outcome.filter(|_| !dry) else {
        return Ok(EventReply::none());
    };
    let mut patch = json!({ "content": outcome.content });
    if let Some(details) = record.and_then(|r| expand::details_patch(event.details.as_ref(), r)) {
        patch["details"] = details;
    }
    Ok(EventReply::patch(patch))
}

/// The `details.toolResults` record of `id` in the session entries, if any.
fn find_record(value: &Value, id: &str) -> Option<Value> {
    match value {
        Value::Object(map) => {
            if let Some(record) = map.get(DETAILS_KEY).filter(|r| r["resultId"] == id) {
                return Some(record.clone());
            }
            map.values().find_map(|v| find_record(v, id))
        }
        Value::Array(items) => items.iter().find_map(|v| find_record(v, id)),
        _ => None,
    }
}

/// The stored text of `id`, else the copy in the session file (`embedOriginal`), else why not.
async fn lookup(host: &Host, id: &str) -> std::result::Result<String, String> {
    let store = store_of(host);
    let key = id.to_owned();
    let text = tokio::task::spawn_blocking(move || store.load(&key))
        .await
        .ok()
        .flatten();
    if let Some(text) = text {
        return Ok(text);
    }
    let entries = host.entries().await.unwrap_or_default();
    let record = find_record(&Value::Array(entries), id);
    match record {
        Some(record) => match record["original"].as_str() {
            Some(text) => Ok(text.to_owned()),
            None => Err(format!(
                "Result {id} ({} characters, {}) is no longer stored: the retention policy or a purge removed it.",
                group(record["chars"].as_u64().unwrap_or(0) as usize),
                record["hash"].as_str().unwrap_or("no hash")
            )),
        },
        None => Err(format!("No stored result {id:?}.")),
    }
}

pub(crate) async fn get_tool(
    state: Shared,
    args: retrieve::Args,
    ctx: ToolCtx,
) -> Result<ToolResult> {
    let config = lock(&state).config.clone();
    let id = args.result_id.trim().to_owned();
    let text = match lookup(ctx.host(), &id).await {
        Ok(text) => text,
        Err(why) => return Ok(ToolResult::error(why)),
    };
    match args.mode.as_deref().unwrap_or("lines") {
        "full" => Ok(ToolResult::text(text)),
        "lines" => Ok(ToolResult::text(retrieve::lines(
            &text, &id, args.start, args.end,
        ))),
        "search" => {
            let Some(pattern) = args.pattern.as_deref().filter(|p| !p.is_empty()) else {
                return Ok(ToolResult::error("mode \"search\" needs a pattern."));
            };
            let query = retrieve::Search {
                pattern,
                regex: args.regex,
                ignore_case: args.ignore_case,
                context: args.context_chars.unwrap_or(config.context_chars),
                max: args.max_matches.unwrap_or(config.max_matches),
            };
            Ok(match retrieve::search(&text, &id, &query) {
                Ok(out) => ToolResult::text(out),
                Err(err) => ToolResult::error(err),
            })
        }
        other => Ok(ToolResult::error(format!(
            "Unknown mode {other:?}; use lines, search, or full."
        ))),
    }
}

/// `render.toolResultExpanded`: the full text of a changed result, for the TUI only.
pub(crate) async fn expanded(req: ToolResultRender, host: Host) -> Result<Option<Vec<String>>> {
    let record = &req.result["details"][DETAILS_KEY];
    let id = record["resultId"].as_str().unwrap_or_default().to_owned();
    if let Some(text) = record["original"].as_str() {
        return Ok(Some(expand::view(&id, Some(text), req.width)));
    }
    let store = store_of(&host);
    let key = id.clone();
    let text = tokio::task::spawn_blocking(move || store.load(&key))
        .await
        .ok()
        .flatten();
    Ok(Some(expand::view(&id, text.as_deref(), req.width)))
}

/// `/tool-results list`: the stored results, newest first.
pub(crate) fn list_text(store: &Store) -> String {
    let all = store.list();
    if all.is_empty() {
        return format!(
            "tool-results: no stored results in {}.",
            store.dir.display()
        );
    }
    let bytes: u64 = all.iter().map(|m| m.bytes).sum();
    let mut out = vec![format!(
        "tool-results: {} stored results, {} bytes, in {}",
        group(all.len()),
        short(bytes as usize),
        store.dir.display()
    )];
    let now = store::now_secs();
    for meta in all.iter().rev().take(50) {
        let age = now.saturating_sub(meta.created);
        let age = match age {
            0..3600 => format!("{}m", age / 60),
            3600..86_400 => format!("{}h", age / 3600),
            _ => format!("{}d", age / 86_400),
        };
        let session = meta.session_id.as_deref().unwrap_or("-");
        out.push(format!(
            "  {}  {:<16} {:>7} chars  {age:>4} ago  session {session}",
            meta.id,
            meta.tool,
            short(meta.chars)
        ));
    }
    if all.len() > 50 {
        out.push(format!("  and {} older", group(all.len() - 50)));
    }
    out.join("\n")
}

/// `/tool-results purge <id|session|all>`: the ids it removes.
pub(crate) fn purge(store: &Store, what: &str, snapshot: &Snapshot) -> Vec<String> {
    let all = store.list();
    let chosen: Vec<&Meta> = match what {
        "all" => all.iter().collect(),
        "session" => all
            .iter()
            .filter(|m| {
                let same_id = m.session_id.is_some() && m.session_id == snapshot.session_id;
                let same_file = m.session_file.is_some() && m.session_file == snapshot.session_file;
                same_id || same_file
            })
            .collect(),
        id => all.iter().filter(|m| m.id == id).collect(),
    };
    chosen
        .into_iter()
        .filter(|m| store.remove(&m.id))
        .map(|m| m.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_records_in_session_entries() {
        let entries = json!([
            { "type": "message", "message": { "role": "toolResult", "details": { "toolResults": { "resultId": "tr-1", "chars": 9 } } } },
            { "type": "message", "message": { "role": "toolResult", "details": { "toolResults": { "resultId": "tr-2", "original": "x" } } } }
        ]);
        assert_eq!(find_record(&entries, "tr-1").unwrap()["chars"], 9);
        assert_eq!(find_record(&entries, "tr-2").unwrap()["original"], "x");
        assert_eq!(find_record(&entries, "tr-3"), None);
    }
}
