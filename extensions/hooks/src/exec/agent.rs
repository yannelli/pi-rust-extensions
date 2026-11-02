//! `agent` handlers: a child `pi -p` run with read, search, and bash tools returns `{ ok, reason }`.
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use super::command::{CommandSpec, run_command};
use super::prompt::{prompt_text, reply_to_run};
use crate::output::RawRun;

const SYSTEM: &str = "You verify a condition for a coding agent's hook. Investigate with the tools \
as needed, then end with only a JSON object: {\"ok\": true} when the condition holds, or \
{\"ok\": false, \"reason\": \"...\"} with what must change.";
const TOOLS: &str = "read,grep,find,ls,bash";
const MAX_TURNS: &str = "50";

/// `$PI_BIN`, else the parent process when it is pi, else `pi` on `PATH`.
pub fn pi_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("PI_BIN").filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    parent_pi().unwrap_or_else(|| PathBuf::from("pi"))
}

fn parent_pi() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        let pid = std::os::unix::process::parent_id();
        let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
        let name = exe.file_name()?.to_str()?.to_ascii_lowercase();
        (name.starts_with("pi") && exe.is_file()).then_some(exe)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Arguments of the child run.
pub fn agent_args(handler: &Value, input: &str) -> Vec<String> {
    let template = handler
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut args = vec![
        "-p".to_owned(),
        "--no-session".to_owned(),
        "--tools".to_owned(),
        TOOLS.to_owned(),
        "--max-turns".to_owned(),
        MAX_TURNS.to_owned(),
        "--append-system-prompt".to_owned(),
        SYSTEM.to_owned(),
    ];
    if let Some(model) = handler
        .get("model")
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty())
    {
        args.push("--model".to_owned());
        args.push(model.to_owned());
    }
    args.push(prompt_text(template, input));
    args
}

pub async fn run_agent(
    handler: &Value,
    event: &str,
    input: String,
    cwd: &Path,
    timeout: Duration,
) -> RawRun {
    let spec = CommandSpec {
        command: pi_binary().display().to_string(),
        args: Some(agent_args(handler, &input)),
        stdin: String::new(),
        cwd: cwd.to_path_buf(),
        env: Vec::new(),
        timeout,
        login_shell: false,
    };
    let run = run_command(spec).await;
    if run.code != Some(0) {
        return run;
    }
    // Agent hooks block like prompt hooks with `continueOnBlock: true`, without `impossible`.
    let reply = strip_impossible(&run.stdout);
    reply_to_run(&reply, event, true)
}

fn strip_impossible(text: &str) -> String {
    text.replace("\"impossible\": true", "\"impossible\": false")
        .replace("\"impossible\":true", "\"impossible\":false")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn child_args_carry_the_prompt_and_model() {
        let handler =
            json!({ "type": "agent", "prompt": "Check $ARGUMENTS", "model": "anthropic/x" });
        let args = agent_args(&handler, "{}");
        assert_eq!(args.last().map(String::as_str), Some("Check {}"));
        assert!(args.windows(2).any(|w| w == ["--model", "anthropic/x"]));
        assert!(args.windows(2).any(|w| w == ["--tools", TOOLS]));
    }

    #[test]
    fn impossible_is_ignored() {
        let reply = strip_impossible(r#"{"ok": false, "reason": "r", "impossible": true}"#);
        assert!(reply_to_run(&reply, "Stop", true).stdout.contains("block"));
    }
}
