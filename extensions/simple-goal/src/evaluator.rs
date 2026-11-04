//! Judges whether the objective is done: a TypeSafe decision when the `typesafe` extension is
//! enabled and `judge` allows it, else one `llm.complete` call.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use pi_ext_sdk::{ExecRequest, Host, LlmRequest};
use serde_json::{Value, json};

use crate::config::{Config, Judge};
use crate::state::Goal;

pub const DEFAULT_SYSTEM: &str = "You evaluate whether a coding agent finished a small task. Judge only from the evidence given. Say done only when the objective is fully met. Reply with one to three sentences of reason, then `<done/>` or `<continue/>` alone on the last line.";
const REASON_LIMIT: usize = 600;
/// Assistant messages the evaluator sees, newest last.
pub const RECENT_MESSAGES: usize = 3;
const MESSAGE_LIMIT: usize = 2_000;
const STATUS_LIMIT: usize = 2_000;
const DIFF_LIMIT: usize = 16_000;

pub const TYPESAFE_TOOL: &str = "typesafe_evaluate";
pub const TYPESAFE_REQUEST: &str = "typesafe:decision:request";
pub const TYPESAFE_REPLY: &str = "typesafe:decision:reply";
/// Above the TypeSafe request timeout of 15 seconds.
const TYPESAFE_WAIT: Duration = Duration::from_secs(20);
const DONE_QUESTION: &str = "done";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub done: bool,
    pub reason: String,
}

/// What the evaluator judges from besides the goal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Evidence {
    /// The text of the last assistant messages, newest last.
    pub messages: Vec<String>,
    pub git_status: String,
    pub git_diff: String,
}

/// The marker must end the last non-empty line; anything else counts as `continue`.
pub fn parse_verdict(output: &str) -> Verdict {
    let trimmed = output.trim_end();
    let (done, body) = if let Some(body) = trimmed.strip_suffix("<done/>") {
        (true, body)
    } else if let Some(body) = trimmed.strip_suffix("<continue/>") {
        (false, body)
    } else {
        (false, trimmed)
    };
    let body = body.trim();
    let mut reason: String = body.chars().take(REASON_LIMIT).collect();
    if body.chars().count() > REASON_LIMIT {
        reason.push('…');
    }
    if reason.is_empty() {
        reason = "no reason given".into();
    }
    Verdict { done, reason }
}

/// `text` cut to `limit` characters, with a note of what was cut.
fn cut(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        return text.to_owned();
    }
    let kept: String = text.chars().take(limit).collect();
    format!("{kept}… ({} more characters)", count - limit)
}

pub fn build_prompt(goal: &Goal, config: &Config, evidence: &Evidence) -> String {
    let mut parts = vec![format!(
        "<objective>\n{}\n</objective>",
        goal.objective.trim()
    )];
    if let Some(done_when) = &goal.done_when {
        parts.push(format!("<done_when>\n{}\n</done_when>", done_when.trim()));
    }
    let messages = if evidence.messages.is_empty() {
        "(none)".to_owned()
    } else {
        evidence.messages.join("\n\n---\n\n")
    };
    parts.push(format!(
        "<last_agent_messages>\n{messages}\n</last_agent_messages>"
    ));
    if !evidence.git_status.trim().is_empty() {
        parts.push(format!(
            "<git_status>\n{}\n</git_status>",
            evidence.git_status.trim()
        ));
    }
    if !evidence.git_diff.trim().is_empty() {
        parts.push(format!(
            "<git_diff>\n{}\n</git_diff>",
            evidence.git_diff.trim()
        ));
    }
    if let Some(instructions) = &config.instructions {
        parts.push(instructions.trim().to_owned());
    }
    parts.push("Is the objective complete? Give a short reason, then end with `<done/>` or `<continue/>` alone on the last line.".into());
    parts.join("\n\n")
}

/// The text of the last `RECENT_MESSAGES` assistant messages on the branch, newest last.
pub fn recent_messages(entries: &[Value]) -> Vec<String> {
    let mut messages: Vec<String> = entries
        .iter()
        .rev()
        .filter_map(|entry| entry.get("message"))
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .filter_map(|message| {
            let text = match message.get("content") {
                Some(Value::String(text)) => text.clone(),
                Some(Value::Array(blocks)) => blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => return None,
            };
            let text = text.trim();
            (!text.is_empty()).then(|| cut(text, MESSAGE_LIMIT))
        })
        .take(RECENT_MESSAGES)
        .collect();
    messages.reverse();
    messages
}

async fn git(host: &Host, args: &[&str]) -> Option<String> {
    let request = ExecRequest::new("git", args)
        .cwd(host.cwd().display().to_string())
        .timeout_ms(10_000)
        .max_buffer(4 * 1024 * 1024);
    let output = host.exec_with(request).await.ok()?;
    output.success().then_some(output.stdout)
}

/// `git status --short` and the diff of the work tree against `HEAD` (or the unstaged diff
/// in a repository without commits).
async fn git_evidence(host: &Host) -> (String, String) {
    let status = git(host, &["status", "--short"]).await.unwrap_or_default();
    let diff = match git(host, &["diff", "HEAD"]).await {
        Some(diff) => diff,
        None => git(host, &["diff"]).await.unwrap_or_default(),
    };
    (
        cut(status.trim_end(), STATUS_LIMIT),
        cut(diff.trim_end(), DIFF_LIMIT),
    )
}

pub async fn gather(host: &Host) -> Evidence {
    let branch = host.branch(None).await.unwrap_or_default();
    let (git_status, git_diff) = git_evidence(host).await;
    Evidence {
        messages: recent_messages(&branch),
        git_status,
        git_diff,
    }
}

/// The TypeSafe request: one `noul` question over the goal and the evidence.
pub fn typesafe_request(goal: &Goal, evidence: &Evidence) -> Value {
    let mut state = json!({
        "objective": goal.objective,
        "last_agent_messages": evidence.messages,
        "git_status": evidence.git_status,
        "git_diff": evidence.git_diff,
    });
    if let Some(done_when) = &goal.done_when {
        state["done_when"] = json!(done_when);
    }
    json!({
        "state": state,
        "questions": {
            DONE_QUESTION: {
                "type": "noul",
                "instructions": "Is the objective fully met, including done_when when given, according to the last agent messages and the git changes?",
                "criteria": { "true": "The objective is fully met", "false": "Work remains or the evidence does not show it is done" }
            }
        }
    })
}

/// The verdict of a TypeSafe reply `result`: done when the probability reaches `threshold`.
pub fn typesafe_verdict(result: &Value, threshold: f64) -> Result<Verdict, String> {
    let probability = result["answers"][DONE_QUESTION]["noul"]
        .as_f64()
        .ok_or("TypeSafe returned no answer to the done question")?;
    let model = result["model"].as_str().unwrap_or("TypeSafe");
    Ok(Verdict {
        done: probability >= threshold,
        reason: format!(
            "{model} judged the objective done with probability {probability:.2} (threshold {threshold:.2})."
        ),
    })
}

async fn typesafe_available(host: &Host) -> bool {
    host.all_tools().await.is_ok_and(|tools| {
        tools
            .iter()
            .any(|tool| tool.get("name").and_then(Value::as_str) == Some(TYPESAFE_TOOL))
    })
}

/// Asks the `typesafe` extension over `pi.events`; `Err` when it is missing, disabled, or fails.
async fn ask_typesafe(host: &Host, request: Value, threshold: f64) -> Result<Verdict, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    if !typesafe_available(host).await {
        return Err("the typesafe extension is not loaded".into());
    }
    let id = format!(
        "simple-goal-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let mut replies = host
        .subscribe(TYPESAFE_REPLY)
        .await
        .map_err(|err| err.message().to_owned())?;
    host.emit(TYPESAFE_REQUEST, json!({ "id": id, "request": request }))
        .await
        .map_err(|err| err.message().to_owned())?;
    let reply = tokio::time::timeout(TYPESAFE_WAIT, async {
        while let Some(reply) = replies.recv().await {
            if reply["id"] == id.as_str() {
                return Some(reply);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
    .ok_or("TypeSafe did not answer in time")?;
    if reply["ok"] != true {
        let code = reply["code"].as_str().unwrap_or("error");
        let error = reply["error"].as_str().unwrap_or("no message");
        return Err(format!("TypeSafe {code}: {error}"));
    }
    typesafe_verdict(&reply["result"], threshold)
}

async fn ask_llm(
    host: &Host,
    goal: &Goal,
    config: &Config,
    evidence: &Evidence,
) -> Result<Verdict, String> {
    let system = config.system_prompt.as_deref().unwrap_or(DEFAULT_SYSTEM);
    let prompt = build_prompt(goal, config, evidence);
    let mut request = LlmRequest::prompt(prompt).system(system).max_tokens(4_000);
    if let Some(model) = &config.model {
        request = request.model(model.clone());
    }
    if let Some(level) = &config.thinking {
        request = request.reasoning(level.clone());
    }
    match host.complete(request).await {
        Ok(response) => Ok(parse_verdict(&response.text)),
        Err(err) => Err(err.message().to_owned()),
    }
}

/// Runs the evaluator; `Err` carries why no verdict came back.
pub async fn evaluate(host: &Host, goal: &Goal, config: &Config) -> Result<Verdict, String> {
    let evidence = gather(host).await;
    if config.judge == Judge::Llm {
        return ask_llm(host, goal, config, &evidence).await;
    }
    let request = typesafe_request(goal, &evidence);
    match ask_typesafe(host, request, config.typesafe_threshold).await {
        Ok(verdict) => Ok(verdict),
        Err(err) if config.judge == Judge::Typesafe => Err(err),
        Err(err) => {
            pi_ext_sdk::debug!("simple-goal: using the LLM evaluator ({err})");
            ask_llm(host, goal, config, &evidence).await
        }
    }
}
