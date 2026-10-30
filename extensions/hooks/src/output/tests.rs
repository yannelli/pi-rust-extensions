use serde_json::json;

use super::*;
use crate::decide::fold_tool;

fn run(source: Source, event: &str, code: i32, stdout: &str, stderr: &str) -> Outcome {
    let cx = Interpret {
        source,
        event,
        tool: Some("Bash"),
        label: "check.sh",
        on_failure_block: false,
    };
    interpret(&cx, &RawRun::exit(code, stdout, stderr))
}

const C: Source = Source::Claude;
const X: Source = Source::Codex;

#[test]
fn exit_codes() {
    let out = run(C, "PreToolUse", 2, "", "no rm");
    assert_eq!(
        out.block.as_deref(),
        Some("PreToolUse:Bash hook error: [check.sh]: no rm")
    );
    assert_eq!(
        run(X, "PreToolUse", 2, "", "no rm").block.as_deref(),
        Some("no rm")
    );
    let ok = run(C, "PreToolUse", 0, "", "");
    assert_eq!(
        ok,
        Outcome {
            source: Some(C),
            ..Outcome::default()
        }
    );
    let other = run(C, "PreToolUse", 1, "", "boom");
    assert!(other.block.is_none());
    assert_eq!(other.error.as_deref(), Some("exit 1: boom"));
    let session = run(C, "SessionStart", 2, "", "x");
    assert!(session.block.is_none() && session.error.is_some());
    let post = run(C, "PostToolUse", 2, "", "look again");
    assert_eq!(post.feedback.as_deref(), Some("look again"));
    assert!(post.block.is_none());
    let cx = Interpret {
        source: C,
        event: "Stop",
        tool: None,
        label: "s",
        on_failure_block: true,
    };
    let failed = interpret(&cx, &RawRun::exit(1, "", "lint failed"));
    assert_eq!(failed.block.as_deref(), Some("lint failed"));
    let codex_fail = Interpret { source: X, ..cx };
    assert!(
        interpret(&codex_fail, &RawRun::exit(1, "", "x"))
            .block
            .is_none()
    );
    let timeout = interpret(&cx, &RawRun::failed("timed out after 1s"));
    assert_eq!(
        timeout.block.as_deref(),
        Some("hook failed (timed out after 1s)")
    );
}

#[test]
fn exit2_json_cannot_override_block() {
    let out = run(
        C,
        "PreToolUse",
        2,
        r#"{"hookSpecificOutput":{"permissionDecision":"allow"},"systemMessage":"hi"}"#,
        "e",
    );
    assert!(out.block.is_some());
    assert_eq!(out.permission, None);
    assert_eq!(out.system_message.as_deref(), Some("hi"));
    let other = run(
        C,
        "PreToolUse",
        1,
        r#"{"permissionDecision":"deny","permissionDecisionReason":"r"}"#,
        "",
    );
    assert_eq!(other.permission, Some((Permission::Deny, Some("r".into()))));
    assert!(
        run(X, "PreToolUse", 1, r#"{"permissionDecision":"deny"}"#, "")
            .permission
            .is_none()
    );
}

#[test]
fn permission_decisions_and_precedence() {
    let allow = run(
        C,
        "PreToolUse",
        0,
        r#"{"hookSpecificOutput":{"permissionDecision":"allow","updatedInput":{"command":"ls"}}}"#,
        "",
    );
    let ask = run(
        C,
        "PreToolUse",
        0,
        r#"{"permissionDecision":"ask","permissionDecisionReason":"sure?"}"#,
        "",
    );
    let defer = run(
        C,
        "PreToolUse",
        0,
        r#"{"hookSpecificOutput":{"permissionDecision":"defer"}}"#,
        "",
    );
    let deny = run(
        C,
        "PreToolUse",
        0,
        r#"{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"nope"}}"#,
        "",
    );
    let fold =
        |list: &[&Outcome]| fold_tool(&list.iter().map(|o| (*o).clone()).collect::<Vec<_>>());
    assert_eq!(
        fold(&[&allow, &ask]).permission,
        Some((Permission::Ask, "sure?".into()))
    );
    assert_eq!(
        fold(&[&ask, &defer, &allow]).permission.unwrap().0,
        Permission::Defer
    );
    assert_eq!(
        fold(&[&allow, &deny, &defer]).permission,
        Some((Permission::Deny, "nope".into()))
    );
    assert_eq!(
        fold(&[&allow]).updated_input,
        Some(json!({ "command": "ls" }))
    );
    let legacy = run(
        C,
        "PreToolUse",
        0,
        r#"{"decision":"block","reason":"old"}"#,
        "",
    );
    assert_eq!(
        legacy.permission,
        Some((Permission::Deny, Some("old".into())))
    );
    let approve = run(C, "PreToolUse", 0, r#"{"decision":"approve"}"#, "");
    assert_eq!(approve.permission.unwrap().0, Permission::Allow);
    let codex_ask = run(X, "PreToolUse", 0, r#"{"permissionDecision":"ask"}"#, "");
    assert!(codex_ask.permission.is_none() && codex_ask.error.is_some());
    let codex_input = run(
        X,
        "PreToolUse",
        0,
        r#"{"updatedInput":{"command":"x"}}"#,
        "",
    );
    assert!(
        codex_input.updated_input.is_none(),
        "Codex needs allow for updatedInput"
    );
    let codex_stop = run(
        X,
        "PreToolUse",
        0,
        r#"{"continue":false,"stopReason":"s"}"#,
        "",
    );
    assert!(codex_stop.stop.is_none() && codex_stop.error.is_some());
}

#[test]
fn permission_request_decision() {
    let deny = run(
        C,
        "PermissionRequest",
        0,
        r#"{"hookSpecificOutput":{"decision":{"behavior":"deny","message":"m"}}}"#,
        "",
    );
    assert_eq!(deny.permission, Some((Permission::Deny, Some("m".into()))));
    let codex = run(
        X,
        "PermissionRequest",
        0,
        r#"{"decision":{"behavior":"allow"}}"#,
        "",
    );
    assert_eq!(codex.permission, Some((Permission::Allow, None)));
    assert!(run(C, "PermissionRequest", 2, "", "x").block.is_none());
}

#[test]
fn post_tool_and_stop_blocks() {
    let block = r#"{"decision":"block","reason":"fix lint","hookSpecificOutput":{"additionalContext":"ctx"}}"#;
    for source in [C, X] {
        let out = run(source, "PostToolUse", 0, block, "");
        assert_eq!(out.block.as_deref(), Some("fix lint"));
        assert_eq!(out.context.as_deref(), Some("ctx"));
    }
    let updated = run(
        C,
        "PostToolUse",
        0,
        r#"{"hookSpecificOutput":{"updatedToolOutput":"new"}}"#,
        "",
    );
    assert_eq!(updated.updated_output, Some(json!("new")));
    assert!(
        run(X, "PostToolUse", 0, r#"{"updatedToolOutput":"new"}"#, "")
            .updated_output
            .is_none()
    );
    let stop = run(
        C,
        "Stop",
        0,
        r#"{"decision":"block","reason":"keep going"}"#,
        "",
    );
    assert_eq!(stop.block.as_deref(), Some("keep going"));
    assert_eq!(run(X, "Stop", 2, "", "more").block.as_deref(), Some("more"));
    let cont = run(
        C,
        "Stop",
        0,
        r#"{"continue":false,"stopReason":"done","decision":"block","reason":"r"}"#,
        "",
    );
    assert_eq!(cont.stop.as_deref(), Some("done"));
    assert_eq!(
        run(X, "Stop", 0, "keep going", "").error.as_deref(),
        Some("plain stdout is invalid here; JSON required")
    );
}

#[test]
fn plain_stdout_context_rules() {
    let cases = [
        (C, "UserPromptSubmit", 0, true),
        (C, "SessionStart", 0, true),
        (C, "PostModelSwitch", 0, true),
        (C, "SubagentStart", 0, false),
        (X, "SubagentStart", 0, true),
        (X, "PostModelSwitch", 0, false),
        (C, "PostToolUse", 0, false),
        (C, "UserPromptSubmit", 1, false),
    ];
    for (source, event, code, want) in cases {
        let context = run(source, event, code, "ctx\n", "").context;
        assert_eq!(context.is_some(), want, "{source:?} {event} exit {code}");
    }
}

#[test]
fn json_detection_and_universal_fields() {
    assert!(json_output(" {\"a\":1} \n").is_some());
    assert!(json_output("{\"a\":1} trailing").is_none());
    assert!(json_output("{not json}").is_none());
    assert!(json_output("[1]").is_none());
    let plain = run(C, "UserPromptSubmit", 0, "{not json}", "");
    assert_eq!(plain.context.as_deref(), Some("{not json}"));
    let out = run(
        C,
        "UserPromptSubmit",
        0,
        r#"{"systemMessage":"warn","suppressOutput":true,"terminalSequence":"\u0007"}"#,
        "",
    );
    assert_eq!(out.system_message.as_deref(), Some("warn"));
    assert_eq!(out.terminal_sequence.as_deref(), Some("\x07"));
    let stop = run(C, "UserPromptSubmit", 0, r#"{"continue":false}"#, "");
    assert_eq!(stop.stop.as_deref(), Some(""));
    let title = run(
        C,
        "SessionStart",
        0,
        r#"{"hookSpecificOutput":{"additionalContext":"c","sessionTitle":"t","initialUserMessage":"go"}}"#,
        "",
    );
    assert_eq!(
        (title.context.as_deref(), title.session_title.as_deref()),
        (Some("c"), Some("t"))
    );
    assert_eq!(title.initial_user_message.as_deref(), Some("go"));
}
