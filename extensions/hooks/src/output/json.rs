use crate::output::{Interpret, Outcome, Permission};
use serde_json::Value;

pub fn apply_json(cx: &Interpret, json: &Value, out: &mut Outcome) {
    let codex = cx.source.is_codex();
    let specific = json.get("hookSpecificOutput").filter(|v| v.is_object());
    let get = |key: &str| specific.and_then(|s| s.get(key)).or_else(|| json.get(key));
    let text = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_owned);
    let reason = text(json.get("reason"));
    let stop_reason = text(json.get("stopReason"));
    out.system_message = text(json.get("systemMessage"));
    out.terminal_sequence = text(json.get("terminalSequence"));
    if codex && let Some(error) = codex_invalid(cx.event, json, specific) {
        out.error = Some(error);
        return;
    }

    if json.get("continue") == Some(&Value::Bool(false)) {
        out.stop = Some(stop_reason.clone().unwrap_or_default());
    }
    out.context = text(get("additionalContext"));
    let wants_block = json.get("decision").and_then(Value::as_str) == Some("block");
    let block = match (wants_block, reason.clone()) {
        (false, _) => None,
        (true, Some(reason)) if !reason.trim().is_empty() => Some(reason),
        (true, _) if codex => {
            out.error = Some("decision block without a reason".into());
            None
        }
        (true, _) => Some("Blocked by hook".to_owned()),
    };

    match cx.event {
        "PreToolUse" if codex => {
            let spec = |key: &str| specific.and_then(|s| s.get(key));
            let decision = text(spec("permissionDecision"));
            if decision.as_deref() == Some("deny") {
                let reason = text(spec("permissionDecisionReason"));
                out.permission = Some((Permission::Deny, reason));
            } else if block.is_some() {
                out.permission = Some((Permission::Deny, block));
            }
            if decision.as_deref() == Some("allow") {
                out.updated_input = spec("updatedInput").filter(|v| v.is_object()).cloned();
            }
        }
        "PreToolUse" | "PreModelSwitch" => {
            let explicit = text(get("permissionDecision")).and_then(|d| Permission::parse(&d));
            let legacy = text(json.get("decision")).and_then(|d| match d.as_str() {
                "approve" => Some(Permission::Allow),
                "block" => Some(Permission::Deny),
                _ => None,
            });
            let reason = text(get("permissionDecisionReason")).or(reason);
            match explicit.or(legacy) {
                Some(Permission::Defer) if cx.event == "PreModelSwitch" => {}
                Some(permission) => out.permission = Some((permission, reason)),
                None => {}
            }
            if cx.event == "PreToolUse" {
                out.updated_input = get("updatedInput").filter(|v| v.is_object()).cloned();
            }
        }
        "PermissionRequest" => {
            let decision = get("decision").filter(|d| d.is_object());
            let behavior = decision
                .and_then(|d| d.get("behavior"))
                .and_then(Value::as_str);
            let message = decision.and_then(|d| text(d.get("message")));
            match behavior {
                Some("allow") => out.permission = Some((Permission::Allow, message)),
                Some("deny") => out.permission = Some((Permission::Deny, message)),
                _ => {}
            }
            if !codex {
                out.updated_input = decision
                    .and_then(|d| d.get("updatedInput"))
                    .filter(|v| v.is_object())
                    .cloned();
            }
        }
        "PostToolUse" | "PostToolUseFailure" => {
            out.block = block;
            if !codex {
                out.updated_output = get("updatedToolOutput")
                    .or_else(|| get("updatedMCPToolOutput"))
                    .cloned();
            }
        }
        "PreCompact" if codex => {}
        "UserPromptSubmit" | "Stop" | "SubagentStop" | "PostToolBatch" | "PreCompact" => {
            out.block = block;
        }
        "SessionStart" if !codex => {
            out.session_title = text(get("sessionTitle"));
            out.initial_user_message = text(get("initialUserMessage"));
        }
        _ => {}
    }
}

/// Codex parses these outputs but marks the run failed and ignores its decision.
fn codex_invalid(event: &str, json: &Value, specific: Option<&Value>) -> Option<String> {
    let has = |key: &str| json.get(key).is_some_and(|v| !v.is_null());
    let spec = |key: &str| specific.and_then(|s| s.get(key)).filter(|v| !v.is_null());
    let stops = json.get("continue") == Some(&Value::Bool(false));
    let suppress = json.get("suppressOutput") == Some(&Value::Bool(true));
    let unsupported = |what: &str| Some(format!("{event} hook returned unsupported {what}"));
    match event {
        "PreToolUse" | "PermissionRequest" if stops => unsupported("continue:false"),
        "PreToolUse" | "PermissionRequest" if has("stopReason") => unsupported("stopReason"),
        "PreToolUse" | "PermissionRequest" | "PostToolUse" if suppress => {
            unsupported("suppressOutput")
        }
        "PreToolUse" => {
            let decision = spec("permissionDecision").and_then(Value::as_str);
            let reason = spec("permissionDecisionReason")
                .and_then(Value::as_str)
                .filter(|r| !r.trim().is_empty());
            match decision {
                _ if spec("updatedInput").is_some() && decision != Some("allow") => {
                    unsupported("updatedInput without permissionDecision:allow")
                }
                Some("allow") if spec("updatedInput").is_none() => {
                    unsupported("permissionDecision:allow")
                }
                Some("ask") => unsupported("permissionDecision:ask"),
                Some("deny") if reason.is_none() => {
                    Some("PreToolUse hook returned deny without a reason".into())
                }
                _ if json.get("decision").and_then(Value::as_str) == Some("approve") => {
                    unsupported("decision:approve")
                }
                _ => None,
            }
        }
        "PermissionRequest" => {
            let decision = spec("decision");
            let field = |key: &str| {
                decision
                    .and_then(|d| d.get(key))
                    .is_some_and(|v| !v.is_null())
            };
            if field("updatedInput") {
                unsupported("updatedInput")
            } else if field("updatedPermissions") {
                unsupported("updatedPermissions")
            } else if decision.and_then(|d| d.get("interrupt")) == Some(&Value::Bool(true)) {
                unsupported("interrupt:true")
            } else {
                None
            }
        }
        "PostToolUse" if spec("updatedMCPToolOutput").is_some() => {
            unsupported("updatedMCPToolOutput")
        }
        _ => None,
    }
}

/// Allows BEL and OSC 0/1/2/9/99/777 terminated by BEL or ST.
pub fn terminal_sequence_allowed(seq: &str) -> bool {
    if seq == "\x07" {
        return true;
    }
    let Some(body) = seq.strip_prefix("\x1b]") else {
        return false;
    };
    let Some(body) = body
        .strip_suffix('\x07')
        .or_else(|| body.strip_suffix("\x1b\\"))
    else {
        return false;
    };
    let code = body.split(';').next().unwrap_or("");
    matches!(code, "0" | "1" | "2" | "9" | "99" | "777") && !body.contains(['\x1b', '\x07'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_sequences() {
        assert!(terminal_sequence_allowed("\x07"));
        assert!(terminal_sequence_allowed("\x1b]0;title\x07"));
        assert!(terminal_sequence_allowed("\x1b]777;notify;a;b\x1b\\"));
        assert!(!terminal_sequence_allowed("\x1b]52;c;data\x07"));
        assert!(!terminal_sequence_allowed("\x1b[2J"));
        assert!(!terminal_sequence_allowed("\x1b]0;a\x1b[2J\x07"));
    }
}
