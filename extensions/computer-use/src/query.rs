use pi_ext_sdk::{Error, Result, ToolResult};
use serde::Deserialize;
use serde_json::json;

use crate::outline::{self, Node};
use crate::output;
use crate::runtime::Runtime;
use crate::state::{Observation, Store};

pub const SEARCH_LIMIT: usize = 12;

pub fn unavailable(state_id: &str) -> String {
    format!("State '{state_id}' is unavailable or was evicted. Observe the root again.")
}

fn trimmed(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

/// Looks up a saved state before any setup, helper, or host call.
pub fn require_state(store: &Store, state_id: &str) -> Result<Observation> {
    let state_id = state_id.trim();
    if state_id.is_empty() {
        return Err(Error::msg("stateId is required. Call observe_ui first."));
    }
    store
        .state(state_id)
        .cloned()
        .ok_or_else(|| Error::msg(unavailable(state_id)))
}

fn node<'a>(observation: &'a Observation, reference: &str) -> Result<&'a Node> {
    observation
        .outline
        .root
        .find(reference.trim())
        .ok_or_else(|| {
            Error::msg(format!(
                "Outline ref '{}' is not available in the current outline.",
                reference.trim()
            ))
        })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchArgs {
    pub text: Option<String>,
    pub role: Option<String>,
    pub capability: Option<String>,
    #[serde(default)]
    pub state_id: String,
}

pub fn search(rt: &Runtime, args: SearchArgs) -> Result<ToolResult> {
    let observation = require_state(&rt.store(), &args.state_id)?;
    let (text, role, capability) = (
        trimmed(&args.text),
        trimmed(&args.role),
        trimmed(&args.capability),
    );
    if text.is_none() && role.is_none() && capability.is_none() {
        return Err(Error::msg(
            "search_ui requires text, role, or capability. Use observe_ui for a bounded overview.",
        ));
    }
    let (matches, total) = outline::search(
        &observation.outline.root,
        text,
        role,
        capability,
        SEARCH_LIMIT,
    );
    let plural = if total == 1 { "" } else { "es" };
    let mut lines = vec![format!(
        "Found {total} outline match{plural}; returned {}.",
        matches.len()
    )];
    if total > matches.len() {
        lines[0].push_str(&format!(
            " Refine the query to inspect {} additional matches.",
            total - matches.len()
        ));
    }
    for found in &matches {
        let label = found.node.label.as_deref().unwrap_or("(unlabeled)");
        let score = if found.reason == "fuzzy" {
            format!(" {:.2}", found.score)
        } else {
            String::new()
        };
        let quoted = serde_json::to_string(label).unwrap_or_default();
        lines.push(format!(
            "{} {} {quoted} [{}{score}]\n  path: {}",
            found.node.reference, found.node.role, found.reason, found.path
        ));
    }
    let refs: Vec<&str> = matches.iter().map(|m| m.node.reference.as_str()).collect();
    let details = json!({
        "tool": "search_ui", "stateId": observation.capture.state_id, "lookId": observation.outline.look_id,
        "matches": refs, "totalMatches": total, "returned": matches.len(), "hasMore": total > matches.len(),
    });
    let text = output::bound(&mut rt.store(), lines.join("\n"));
    Ok(ToolResult::text(text).details(details))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefArgs {
    #[serde(rename = "ref", default)]
    pub reference: String,
    pub depth: Option<f64>,
    pub offset: Option<f64>,
    #[serde(default)]
    pub state_id: Option<String>,
}

pub fn expand(rt: &Runtime, args: RefArgs) -> Result<ToolResult> {
    let observation = require_state(&rt.store(), args.state_id.as_deref().unwrap_or_default())?;
    if args.reference.trim().is_empty() {
        return Err(Error::msg("expand_ui.ref is required."));
    }
    let target = node(&observation, &args.reference)?;
    let depth = args.depth.unwrap_or(3.0).clamp(1.0, 8.0) as usize;
    let (rendered, omitted) = target.render(depth, output::RENDER_LINES);
    let more = if omitted > 0 {
        format!("\n… {omitted} more nodes. Use search_ui.")
    } else {
        String::new()
    };
    let details = json!({
        "tool": "expand_ui", "stateId": observation.capture.state_id, "lookId": observation.outline.look_id,
        "target": target.reference, "renderedOutline": rendered,
    });
    let text = output::bound(&mut rt.store(), format!("{rendered}{more}"));
    Ok(ToolResult::text(text).details(details))
}

pub fn inspect(rt: &Runtime, args: RefArgs) -> Result<ToolResult> {
    let observation = require_state(&rt.store(), args.state_id.as_deref().unwrap_or_default())?;
    if args.reference.trim().is_empty() {
        return Err(Error::msg("inspect_ui.ref is required."));
    }
    let target = node(&observation, &args.reference)?;
    let details = json!({
        "tool": "inspect_ui", "stateId": observation.capture.state_id,
        "lookId": observation.outline.look_id, "target": target.reference,
    });
    Ok(ToolResult::text(outline::inspect(target)).details(details))
}

pub fn read_text(rt: &Runtime, args: RefArgs) -> Result<ToolResult> {
    let reference = args.reference.trim();
    let offset = args.offset.unwrap_or(0.0).max(0.0) as usize;
    if reference.starts_with("@o") {
        let store = rt.store();
        let text = store.output(reference).ok_or_else(|| {
            Error::msg(format!(
                "Output ref '{reference}' is unavailable or was evicted. Rerun the focused query."
            ))
        })?;
        let details = json!({ "tool": "read_text", "ref": reference, "offset": offset });
        return Ok(ToolResult::text(output::output_page(text, offset)).details(details));
    }
    if reference.is_empty() {
        return Err(Error::msg(
            "read_text requires ref. Call observe_ui/inspect_ui and use a text-bearing outline ref.",
        ));
    }
    let observation = require_state(&rt.store(), args.state_id.as_deref().unwrap_or_default())?;
    let target = node(&observation, reference)?;
    let details = json!({ "tool": "read_text", "stateId": observation.capture.state_id, "ref": reference, "offset": offset });
    Ok(ToolResult::text(output::text_page(&target.text(), offset)).details(details))
}
