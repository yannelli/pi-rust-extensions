use crate::output::{Interpret, Outcome, Permission};
use serde_json::Value;

pub fn apply_json(cx: &Interpret, json: &Value, out: &mut Outcome) {
    let codex = cx.source.is_codex();
    let specific = json.get("hookSpecificOutput").filter(|v| v.is_object());
    let get = |key: &str| specific.and_then(|s| s.get(key)).or_else(|| json.get(key));
    let text = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_owned);
    let reason = text(json.get("reason"));
    let stop_reason = text(json.get("stopReason"));

    if json.get("continue") == Some(&Value::Bool(false)) {
        if codex && cx.event == "PreToolUse" {
            out.error = Some("continue is not supported for PreToolUse".into());
        } else {
            out.stop = Some(stop_reason.clone().unwrap_or_default());
        }
    } else if codex && cx.event == "PreToolUse" && stop_reason.is_some() {
        out.error = Some("stopReason is not supported for PreToolUse".into());
    }
    out.system_message = text(json.get("systemMessage"));
    out.terminal_sequence = text(json.get("terminalSequence"));
    out.context = text(get("additionalContext"));
    let block = (json.get("decision").and_then(Value::as_str) == Some("block")).then(|| {
        reason
            .clone()
            .unwrap_or_else(|| "Blocked by hook".to_owned())
    });

    match cx.event {
        "PreToolUse" => {
            let explicit = text(get("permissionDecision")).and_then(|d| Permission::parse(&d));
            let legacy = text(json.get("decision")).and_then(|d| match d.as_str() {
                "approve" => Some(Permission::Allow),
                "block" => Some(Permission::Deny),
                _ => None,
            });
            let reason = text(get("permissionDecisionReason")).or(reason);
            match explicit.or(legacy) {
                Some(Permission::Ask) if codex => out.error = Some("ask is not supported".into()),
                Some(permission) => out.permission = Some((permission, reason)),
                None => {}
            }
            let allowed = matches!(out.permission, Some((Permission::Allow, _)));
            if !codex || allowed {
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
            out.updated_input = decision
                .and_then(|d| d.get("updatedInput"))
                .filter(|v| v.is_object())
                .cloned();
        }
        "PostToolUse" | "PostToolUseFailure" => {
            out.block = block;
            if !codex {
                out.updated_output = get("updatedToolOutput")
                    .or_else(|| get("updatedMCPToolOutput"))
                    .cloned();
            }
        }
        "UserPromptSubmit" | "Stop" | "SubagentStop" | "PostToolBatch" | "PreCompact"
        | "PreModelSwitch" => {
            out.block = block;
        }
        "SessionStart" if !codex => {
            out.session_title = text(get("sessionTitle"));
            out.initial_user_message = text(get("initialUserMessage"));
        }
        _ => {}
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
