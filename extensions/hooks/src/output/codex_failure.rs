use crate::output::Outcome;

/// Codex `onFailure: "block"` (0.173.0-alpha): a failed run sends the event's own block or stop.
pub fn apply_codex_failure(event: &str, out: &mut Outcome) {
    if out.error.is_none() {
        return;
    }
    let reason =
        format!("{event} hook failed. This hook is configured to block continuation on failure.");
    match event {
        "PreToolUse" | "PermissionRequest" | "PostToolUse" | "Stop" | "SubagentStop" => {
            out.block = Some(reason);
            out.permission = None;
            out.updated_input = None;
        }
        "UserPromptSubmit" | "SessionStart" | "SubagentStart" | "PreCompact" | "PostCompact" => {
            out.stop = Some(reason);
        }
        _ => {}
    }
}

/// Codex does not load a `block` handler that cannot block: SessionEnd, model request events,
/// and async commands.
pub fn codex_failure_unenforceable(event: &str, async_command: bool) -> bool {
    async_command || matches!(event, "SessionEnd" | "PreModelRequest" | "PostModelRequest")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Source;
    use crate::output::{Interpret, Permission, RawRun, interpret};

    fn codex(event: &str, raw: RawRun) -> Outcome {
        let cx = Interpret {
            source: Source::Codex,
            event,
            tool: Some("Bash"),
            label: "check.sh",
            on_failure_block: true,
            kind: "command",
            plugin: false,
        };
        interpret(&cx, &raw)
    }

    fn reason(event: &str) -> String {
        format!("{event} hook failed. This hook is configured to block continuation on failure.")
    }

    #[test]
    fn failures_block_or_stop_by_event() {
        for event in [
            "PreToolUse",
            "PermissionRequest",
            "PostToolUse",
            "Stop",
            "SubagentStop",
        ] {
            let out = codex(event, RawRun::exit(1, "", "boom"));
            assert_eq!(out.block, Some(reason(event)), "{event}");
            assert_eq!(out.error.as_deref(), Some("exit 1: boom"));
        }
        for event in [
            "UserPromptSubmit",
            "SessionStart",
            "SubagentStart",
            "PreCompact",
            "PostCompact",
        ] {
            let out = codex(event, RawRun::failed("timed out after 1s"));
            assert_eq!(out.stop, Some(reason(event)), "{event}");
            assert!(out.block.is_none());
        }
        let interrupt = codex("Interrupt", RawRun::exit(1, "", ""));
        assert!(interrupt.block.is_none() && interrupt.stop.is_none());
    }

    #[test]
    fn codex_failed_runs_count() {
        let blocks = |event: &str, raw: RawRun| codex(event, raw).decision().is_some();
        assert!(blocks("PreToolUse", RawRun::exit(2, "", "")));
        assert!(blocks("PreToolUse", RawRun::exit(0, "{invalid json", "")));
        assert!(blocks(
            "PreToolUse",
            RawRun::exit(0, r#"{"continue":false}"#, "")
        ));
        assert!(blocks(
            "PermissionRequest",
            RawRun::exit(
                0,
                r#"{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow","interrupt":true}}}"#,
                ""
            )
        ));
        assert!(blocks("Stop", RawRun::exit(0, "hook finished", "")));
        assert!(blocks(
            "Stop",
            RawRun::exit(0, r#"{"decision":"block"}"#, "")
        ));
        assert_eq!(
            codex("SessionStart", RawRun::exit(2, "", "x")).stop,
            Some(reason("SessionStart"))
        );
        assert!(!blocks("PreToolUse", RawRun::exit(0, "", "")));
        assert!(!blocks("PreToolUse", RawRun::exit(0, "{}", "")));
        assert!(!blocks("PreToolUse", RawRun::exit(0, "hook finished", "")));
        assert!(blocks(
            "PermissionRequest",
            RawRun::exit(
                0,
                r#"{"hookSpecificOutput":{"decision":{"behavior":"allow"}}}"#,
                ""
            )
        ));
        let allowed = r#"{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}"#;
        assert_eq!(
            codex("PermissionRequest", RawRun::exit(0, allowed, "")).decision(),
            Some((Permission::Allow, None))
        );
        let denied = codex("PreToolUse", RawRun::exit(2, "", "no rm"));
        assert_eq!(denied.block.as_deref(), Some("no rm"));
    }

    #[test]
    fn unenforceable_handlers() {
        assert!(codex_failure_unenforceable("SessionEnd", false));
        assert!(codex_failure_unenforceable("PostModelRequest", false));
        assert!(codex_failure_unenforceable("PreToolUse", true));
        assert!(!codex_failure_unenforceable("PreToolUse", false));
    }
}
