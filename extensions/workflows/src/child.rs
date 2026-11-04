//! Runs one step agent (`pi --mode json -p`) and folds its events into a report.

use std::process::Stdio;
use std::time::Duration;

use pi_ext_sdk::CancellationToken;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::launch::Launch;

const STDERR_TAIL: usize = 2000;
const KILL_GRACE: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Done,
    Continue,
    Blocked,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Report { status: Status, summary: String },
    Failed(String),
    Cancelled,
}

/// What the agent did so far, from its JSON events.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fold {
    pub turns: u32,
    pub text: String,
    pub final_text: String,
    pub output: Option<Value>,
    pub error: Option<String>,
    pub limit: Option<String>,
    pub cost: f64,
    pub model: Option<String>,
    /// The running tool, or the start of the text being written.
    pub activity: String,
}

impl Fold {
    /// Applies one event; `true` when something shown changed.
    pub fn apply(&mut self, event: &Value) -> bool {
        match event["type"].as_str().unwrap_or("") {
            "turn_start" => self.turns += 1,
            "message_start" if event["message"]["role"] == "assistant" => self.text.clear(),
            "message_update" => {
                let update = &event["assistantMessageEvent"];
                if update["type"] != "text_delta" {
                    return false;
                }
                self.text.push_str(update["delta"].as_str().unwrap_or(""));
                self.activity = last_line(&self.text);
            }
            "message_end" if event["message"]["role"] == "assistant" => {
                self.end_message(&event["message"])
            }
            "tool_execution_start" => {
                let name = event["toolName"].as_str().unwrap_or("tool");
                self.activity = format!("{name} {}", summarize(&event["args"]));
            }
            "output" => self.output = Some(event["value"].clone()),
            "run_limit" => self.limit = event["message"].as_str().map(str::to_owned),
            _ => return false,
        }
        true
    }

    fn end_message(&mut self, message: &Value) {
        let text = message_text(message);
        if !text.is_empty() {
            self.final_text = text;
        }
        let stop = message["stopReason"].as_str().unwrap_or("");
        self.error = matches!(stop, "error" | "aborted")
            .then(|| message["errorMessage"].as_str().unwrap_or(stop).to_owned());
        self.cost += message["usage"]["cost"]["total"].as_f64().unwrap_or(0.0);
        if let (Some(provider), Some(model)) =
            (message["provider"].as_str(), message["model"].as_str())
        {
            self.model = Some(format!("{provider}/{model}"));
        }
    }

    /// The report for the exit code: the JSON answer, else the last text.
    pub fn outcome(&self, code: Option<i32>, stderr: &str) -> Outcome {
        let report = self.output.as_ref().and_then(|value| {
            let status = match value["status"].as_str()? {
                "done" => Status::Done,
                "continue" => Status::Continue,
                "blocked" => Status::Blocked,
                _ => return None,
            };
            Some((status, value["summary"].as_str().unwrap_or("").to_owned()))
        });
        match (code, report) {
            (Some(0), Some((status, summary))) => Outcome::Report { status, summary },
            // 4: the answer did not match the report schema; the text still is the report.
            (Some(0 | 4), None) if self.error.is_none() => Outcome::Report {
                status: Status::Done,
                summary: self.final_text.clone(),
            },
            (Some(3), _) => Outcome::Failed(
                self.limit
                    .clone()
                    .unwrap_or_else(|| "stopped by a run limit".into()),
            ),
            _ => {
                let reason = self.error.clone().unwrap_or_else(|| {
                    let tail = tail(stderr.trim(), STDERR_TAIL);
                    let code = code.map_or("a signal".to_owned(), |c| format!("code {c}"));
                    if tail.is_empty() {
                        format!("pi exited with {code}")
                    } else {
                        format!("pi exited with {code}: {tail}")
                    }
                });
                Outcome::Failed(reason)
            }
        }
    }
}

/// Runs the agent until it exits or `cancel` fires; `on_change` sees each update.
pub async fn run(
    launch: &Launch,
    cancel: &CancellationToken,
    mut on_change: impl FnMut(&Fold),
) -> (Outcome, Fold) {
    let mut fold = Fold::default();
    let mut command = Command::new(&launch.program);
    command
        .args(&launch.args)
        .envs(launch.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&launch.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            return (
                Outcome::Failed(format!("cannot start {}: {err}", launch.program.display())),
                fold,
            );
        }
    };
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text).await;
            text
        })
    });
    let mut lines = BufReader::new(stdout).lines();
    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                stop(&mut child).await;
                return (Outcome::Cancelled, fold);
            }
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    let event = serde_json::from_str::<Value>(line.trim()).unwrap_or(Value::Null);
                    if fold.apply(&event) {
                        on_change(&fold);
                    }
                }
                _ => break,
            },
        }
    }
    let exit = tokio::select! {
        () = cancel.cancelled() => {
            stop(&mut child).await;
            return (Outcome::Cancelled, fold);
        }
        exit = child.wait() => exit,
    };
    let stderr = match stderr {
        Some(task) => tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default(),
        None => String::new(),
    };
    let code = exit.ok().and_then(|status| status.code());
    (fold.outcome(code, &stderr), fold)
}

/// SIGTERM to the agent's process group, then SIGKILL after a grace period.
async fn stop(child: &mut Child) {
    #[cfg(unix)]
    signal_group(child, "-TERM");
    if tokio::time::timeout(KILL_GRACE, child.wait())
        .await
        .is_err()
    {
        #[cfg(unix)]
        signal_group(child, "-KILL");
        let _ = child.kill().await;
    }
}

#[cfg(unix)]
fn signal_group(child: &Child, signal: &str) {
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("kill")
            .args([signal, "--", &format!("-{pid}")])
            .stderr(Stdio::null())
            .status();
    }
}

pub fn message_text(message: &Value) -> String {
    match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn summarize(args: &Value) -> String {
    let keys = [
        "command",
        "path",
        "file_path",
        "pattern",
        "query",
        "url",
        "action",
        "agent",
    ];
    let text = keys.iter().find_map(|k| args[*k].as_str()).unwrap_or("");
    last_line(text.lines().next().unwrap_or(""))
}

fn last_line(text: &str) -> String {
    let line = text.trim_end().lines().last().unwrap_or("").trim();
    if line.chars().count() > 120 {
        line.chars().take(119).chain(['…']).collect()
    } else {
        line.to_owned()
    }
}

fn tail(text: &str, max: usize) -> &str {
    let mut start = text.len().saturating_sub(max);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn fold(events: &[Value]) -> Fold {
        let mut fold = Fold::default();
        for event in events {
            fold.apply(event);
        }
        fold
    }

    #[test]
    fn report_comes_from_the_output_event() {
        let f = fold(&[
            json!({"type": "turn_start"}),
            json!({"type": "tool_execution_start", "toolName": "bash", "args": {"command": "gh pr checks\nmore"}}),
            json!({"type": "message_end", "message": {"role": "assistant", "content": [{"type": "text", "text": "{...}"}],
                "provider": "anthropic", "model": "haiku", "usage": {"cost": {"total": 0.25}}}}),
            json!({"type": "output", "value": {"status": "continue", "summary": "CI is still running"}}),
        ]);
        assert_eq!(f.activity, "bash gh pr checks");
        assert_eq!(
            (f.turns, f.cost, f.model.as_deref()),
            (1, 0.25, Some("anthropic/haiku"))
        );
        assert_eq!(
            f.outcome(Some(0), ""),
            Outcome::Report {
                status: Status::Continue,
                summary: "CI is still running".into()
            }
        );
    }

    #[test]
    fn exit_codes_map_to_outcomes() {
        let f = fold(&[
            json!({"type": "message_end", "message": {"role": "assistant", "content": "plain report"}}),
        ]);
        assert_eq!(
            f.outcome(Some(4), ""),
            Outcome::Report {
                status: Status::Done,
                summary: "plain report".into()
            }
        );
        let limited =
            fold(&[json!({"type": "run_limit", "message": "stopped after 5 turns (--max-turns)"})]);
        assert_eq!(
            limited.outcome(Some(3), ""),
            Outcome::Failed("stopped after 5 turns (--max-turns)".into())
        );
        assert_eq!(
            Fold::default().outcome(Some(1), "boom\n"),
            Outcome::Failed("pi exited with code 1: boom".into())
        );
        let errored = fold(&[
            json!({"type": "message_end", "message": {"role": "assistant", "content": [], "stopReason": "error", "errorMessage": "429"}}),
        ]);
        assert_eq!(errored.outcome(Some(1), ""), Outcome::Failed("429".into()));
    }
}
