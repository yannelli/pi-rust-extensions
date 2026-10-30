use serde_json::Value;

use crate::output::{Outcome, Permission};

/// PreToolUse or PermissionRequest outcomes folded across hooks.
#[derive(Debug, Default, PartialEq)]
pub struct ToolDecision {
    pub stop: Option<String>,
    pub permission: Option<(Permission, String)>,
    pub updated_input: Option<Value>,
    pub context: Vec<String>,
}

pub fn fold_tool(outcomes: &[Outcome]) -> ToolDecision {
    let decisions: Vec<(Permission, Option<String>)> =
        outcomes.iter().filter_map(Outcome::decision).collect();
    let permission = decisions.iter().map(|(p, _)| *p).max().map(|best| {
        let reason = decisions
            .iter()
            .filter(|(p, _)| *p == best)
            .find_map(|(_, reason)| reason.clone().filter(|r| !r.is_empty()))
            .unwrap_or_else(|| default_reason(best));
        (best, reason)
    });
    ToolDecision {
        stop: first_stop(outcomes),
        permission,
        updated_input: outcomes.iter().rev().find_map(|o| o.updated_input.clone()),
        context: contexts(outcomes),
    }
}

fn default_reason(permission: Permission) -> String {
    match permission {
        Permission::Deny => "Blocked by hook".into(),
        Permission::Ask => "A hook asks to confirm this tool call".into(),
        _ => String::new(),
    }
}

pub fn first_stop(outcomes: &[Outcome]) -> Option<String> {
    outcomes.iter().find_map(|o| o.stop.clone())
}

pub fn first_block(outcomes: &[Outcome]) -> Option<String> {
    outcomes.iter().find_map(|o| o.block.clone())
}

pub fn contexts(outcomes: &[Outcome]) -> Vec<String> {
    outcomes
        .iter()
        .filter_map(|o| o.context.clone())
        .filter(|c| !c.trim().is_empty())
        .collect()
}

/// `stopReason` text, or a default.
pub fn stop_text(reason: &str) -> String {
    if reason.is_empty() {
        "A hook stopped processing".into()
    } else {
        reason.to_owned()
    }
}
