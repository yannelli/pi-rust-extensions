use serde_json::Value;

mod json;

use json::apply_json;
pub use json::terminal_sequence_allowed;

use crate::model::Source;

/// What a handler produced: exit code (`None` when it did not exit), stdout, stderr.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawRun {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// Spawn failure, timeout, or HTTP error.
    pub error: Option<String>,
}

impl RawRun {
    pub fn exit(code: i32, stdout: impl Into<String>, stderr: impl Into<String>) -> Self {
        Self {
            code: Some(code),
            stdout: stdout.into(),
            stderr: stderr.into(),
            error: None,
        }
    }

    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            error: Some(error.into()),
            ..Self::default()
        }
    }
}

/// Ordered so that `max` gives the precedence deny > defer > ask > allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Permission {
    Allow,
    Ask,
    Defer,
    Deny,
}

impl Permission {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "allow" | "approve" => Some(Self::Allow),
            "ask" => Some(Self::Ask),
            "defer" => Some(Self::Defer),
            "deny" | "block" => Some(Self::Deny),
            _ => None,
        }
    }
}

/// One handler result read by the rules of its source and event.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    pub source: Option<Source>,
    pub block: Option<String>,
    pub permission: Option<(Permission, Option<String>)>,
    pub updated_input: Option<Value>,
    pub context: Option<String>,
    /// `continue: false`, with `stopReason`.
    pub stop: Option<String>,
    pub system_message: Option<String>,
    pub terminal_sequence: Option<String>,
    pub updated_output: Option<Value>,
    /// Exit 2 stderr on events that show it to the model without blocking.
    pub feedback: Option<String>,
    pub session_title: Option<String>,
    pub initial_user_message: Option<String>,
    /// `watchPaths` from SessionStart, CwdChanged, or FileChanged.
    pub watch_paths: Option<Vec<String>>,
    /// MessageDisplay `displayContent`.
    pub display_content: Option<String>,
    /// PermissionDenied `retry: true`.
    pub retry: bool,
    /// Elicitation and ElicitationResult `action` with optional `content`.
    pub elicitation: Option<(String, Option<Value>)>,
    /// WorktreeCreate path: `worktreePath`, or the last stdout line of a command hook.
    pub worktree_path: Option<String>,
    pub error: Option<String>,
}

impl Outcome {
    /// The PreToolUse / PermissionRequest decision; a block counts as deny.
    pub fn decision(&self) -> Option<(Permission, Option<String>)> {
        match &self.block {
            Some(reason) => Some((Permission::Deny, Some(reason.clone()))),
            None => self.permission.clone(),
        }
    }

    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(reason) = &self.stop {
            parts.push(format!("stop {reason}"));
        }
        if self.block.is_some() {
            parts.push("block".to_owned());
        }
        if let Some((permission, _)) = &self.permission {
            parts.push(format!("{permission:?}").to_lowercase());
        }
        if self.updated_input.is_some() || self.updated_output.is_some() {
            parts.push("update".to_owned());
        }
        if self.context.is_some() {
            parts.push("context".to_owned());
        }
        if let Some(error) = &self.error {
            parts.push(format!("error: {error}"));
        }
        parts.join(", ")
    }
}

/// Events where exit 2 blocks (Codex PermissionRequest denies, PostToolUse replaces the result).
pub fn exit2_blocks(source: Source, event: &str) -> bool {
    if source.is_codex() {
        return matches!(
            event,
            "PreToolUse"
                | "PermissionRequest"
                | "PostToolUse"
                | "UserPromptSubmit"
                | "Stop"
                | "SubagentStop"
        );
    }
    matches!(
        event,
        "PreToolUse"
            | "UserPromptSubmit"
            | "Stop"
            | "SubagentStop"
            | "PostToolBatch"
            | "PreCompact"
            | "PreModelSwitch"
            | "TaskCreated"
            | "TaskCompleted"
            | "TeammateIdle"
            | "ConfigChange"
            | "UserPromptExpansion"
            | "Elicitation"
            | "ElicitationResult"
    )
}

/// Claude `onFailure: "block"`: like exit 2, plus PermissionRequest; never on Stop/SubagentStop.
fn failure_blocks(event: &str) -> bool {
    event == "PermissionRequest"
        || (exit2_blocks(Source::Claude, event)
            && !matches!(
                event,
                "Stop" | "SubagentStop" | "TaskCompleted" | "TeammateIdle"
            ))
}

fn plain_stdout_is_context(source: Source, event: &str) -> bool {
    if source.is_codex() {
        return matches!(event, "SessionStart" | "UserPromptSubmit" | "SubagentStart");
    }
    matches!(
        event,
        "UserPromptSubmit" | "SessionStart" | "PostModelSwitch" | "UserPromptExpansion"
    )
}

/// Stdout counts as JSON only when it starts with `{`, ends with `}`, and parses.
pub fn json_output(stdout: &str) -> Option<Value> {
    let text = stdout.trim();
    if !(text.starts_with('{') && text.ends_with('}')) {
        return None;
    }
    serde_json::from_str::<Value>(text)
        .ok()
        .filter(Value::is_object)
}

pub struct Interpret<'a> {
    pub source: Source,
    pub event: &'a str,
    pub tool: Option<&'a str>,
    pub label: &'a str,
    pub on_failure_block: bool,
}

pub fn interpret(cx: &Interpret, raw: &RawRun) -> Outcome {
    let mut out = Outcome {
        source: Some(cx.source),
        ..Outcome::default()
    };
    let codex = cx.source.is_codex();
    let json = json_output(&raw.stdout);
    let stderr = raw.stderr.trim();
    let blocks = exit2_blocks(cx.source, cx.event);
    let invalid_json = json.is_none() && looks_like_json(&raw.stdout);
    match raw.code {
        Some(0) => match &json {
            Some(json) => apply_json(cx, json, &mut out),
            None if invalid_json => out.error = Some("invalid JSON output".into()),
            None if cx.event == "WorktreeCreate" => {
                out.worktree_path = last_line(&raw.stdout);
            }
            None if !raw.stdout.trim().is_empty() => {
                if codex && matches!(cx.event, "Stop" | "SubagentStop") {
                    out.error = Some("plain stdout is invalid here; JSON required".into());
                } else if plain_stdout_is_context(cx.source, cx.event) {
                    out.context = Some(raw.stdout.trim().to_owned());
                }
            }
            None => {}
        },
        Some(2) if codex && blocks && stderr.is_empty() => {
            out.error = Some(format!(
                "{} hook exited with code 2 but wrote no reason to stderr",
                cx.event
            ));
        }
        Some(2)
            if !codex
                && matches!(cx.event, "Stop" | "SubagentStop")
                && raw.stdout.trim().is_empty()
                && stderr.contains("No such file or directory") =>
        {
            out.error = Some(format!("exit 2: {stderr}"));
        }
        Some(2) => {
            if !codex && let Some(json) = &json {
                apply_json(cx, json, &mut out);
            }
            if blocks {
                let json_reason = out
                    .block
                    .clone()
                    .or_else(|| match &out.permission {
                        Some((Permission::Deny, reason)) => reason.clone(),
                        _ => None,
                    })
                    .filter(|r| !r.is_empty() && r != "Blocked by hook");
                out.block = Some(json_reason.unwrap_or_else(|| exit2_reason(cx, stderr)));
                out.permission = None;
                out.updated_input = None;
            } else if matches!(cx.event, "PostToolUse" | "PostToolUseFailure") {
                out.feedback = Some(stderr.to_owned()).filter(|s| !s.is_empty());
            } else {
                out.error = Some(format!("exit 2: {stderr}"));
            }
        }
        code => {
            let what = match (code, &raw.error) {
                (_, Some(error)) => error.clone(),
                (Some(code), None) => format!("exit {code}"),
                (None, None) => "killed".to_owned(),
            };
            out.error = Some(if stderr.is_empty() {
                what.clone()
            } else {
                format!("{what}: {stderr}")
            });
            if !codex
                && code.is_some()
                && let Some(json) = &json
            {
                apply_json(cx, json, &mut out);
            }
        }
    }
    let failed = raw.code != Some(0) && raw.code != Some(2) || invalid_json && raw.code == Some(0);
    if cx.on_failure_block && !codex && failed && failure_blocks(cx.event) {
        out.block = Some(failure_reason(cx, raw, stderr));
        out.permission = None;
        out.updated_input = None;
    }
    out
}

/// The last non-empty stdout line without ANSI escapes (WorktreeCreate command hooks).
fn last_line(stdout: &str) -> Option<String> {
    let ansi = regex::Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07").ok()?;
    let clean = ansi.replace_all(stdout, "");
    clean
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(str::to_owned)
}

/// Stdout that a hook meant as JSON: starts with `{` and ends with `}`.
fn looks_like_json(stdout: &str) -> bool {
    let text = stdout.trim();
    text.starts_with('{') && text.ends_with('}')
}

fn failure_reason(cx: &Interpret, raw: &RawRun, stderr: &str) -> String {
    let timed_out = raw
        .error
        .as_deref()
        .is_some_and(|e| e.starts_with("timed out"));
    let what = if timed_out { "timed out" } else { "failed" };
    let detail = [stderr, raw.error.as_deref().unwrap_or_default()]
        .into_iter()
        .find(|t| !t.is_empty())
        .map(|t| format!("\n{t}"))
        .unwrap_or_default();
    let head = match cx.tool {
        Some(tool) => format!("{}:{tool} hook error", cx.event),
        None => format!("{} hook error", cx.event),
    };
    format!(
        "{head}: [{}]: {what}; blocking because onFailure is \"block\"{detail}",
        cx.label
    )
}

fn exit2_reason(cx: &Interpret, stderr: &str) -> String {
    match (cx.event, cx.tool) {
        ("PreToolUse", Some(tool)) if !cx.source.is_codex() => {
            format!("PreToolUse:{tool} hook error: [{}]: {stderr}", cx.label)
        }
        _ if stderr.is_empty() => "Blocked by hook".to_owned(),
        _ => stderr.to_owned(),
    }
}

#[cfg(test)]
mod tests;
