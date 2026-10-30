use std::time::Duration;

use pi_ext_sdk::{Host, LlmRequest};
use serde_json::{Value, json};

use crate::output::RawRun;

const SYSTEM: &str = "You evaluate a hook event for a coding agent. Reply with only a JSON object: \
{\"ok\": true} to allow, or {\"ok\": false, \"reason\": \"...\"} to block.";

pub fn prompt_text(template: &str, input: &str) -> String {
    if template.contains("$ARGUMENTS") {
        template.replace("$ARGUMENTS", input)
    } else {
        format!("{template}\n\n{input}")
    }
}

pub async fn run_prompt(host: &Host, handler: &Value, input: String, timeout: Duration) -> RawRun {
    let template = handler
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut request = LlmRequest::prompt(prompt_text(template, &input)).system(SYSTEM);
    if let Some(model) = handler.get("model").and_then(Value::as_str) {
        request = request.model(model);
    }
    match tokio::time::timeout(timeout, host.complete(request)).await {
        Err(_) => RawRun::failed(format!("timed out after {}s", timeout.as_secs_f64())),
        Ok(Err(err)) => RawRun::failed(format!("llm.complete failed: {err}")),
        Ok(Ok(response)) => reply_to_run(&response.text),
    }
}

/// `{ok, reason}` or `{decision, reason}` -> a run whose stdout the normal rules read.
pub fn reply_to_run(text: &str) -> RawRun {
    let parsed = text
        .find('{')
        .zip(text.rfind('}'))
        .filter(|(start, end)| start < end)
        .and_then(|(start, end)| serde_json::from_str::<Value>(&text[start..=end]).ok());
    let Some(reply) = parsed else {
        return RawRun::exit(1, "", format!("prompt hook reply is not JSON: {text}"));
    };
    let reason = reply
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("Blocked by prompt hook");
    let blocked = reply.get("ok") == Some(&Value::Bool(false))
        || matches!(
            reply.get("decision").and_then(Value::as_str),
            Some("block" | "deny")
        );
    if blocked {
        RawRun::exit(
            0,
            json!({ "decision": "block", "reason": reason }).to_string(),
            "",
        )
    } else {
        RawRun::exit(0, "", "")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_placeholder() {
        assert_eq!(prompt_text("check $ARGUMENTS now", "{}"), "check {} now");
        assert_eq!(prompt_text("check", "{}"), "check\n\n{}");
    }

    #[test]
    fn replies() {
        let block = reply_to_run("```json\n{\"ok\": false, \"reason\": \"tests fail\"}\n```");
        assert_eq!(
            block.stdout,
            r#"{"decision":"block","reason":"tests fail"}"#
        );
        assert_eq!(reply_to_run(r#"{"ok": true}"#).stdout, "");
        assert!(
            reply_to_run(r#"{"decision":"deny"}"#)
                .stdout
                .contains("block")
        );
        assert_eq!(reply_to_run("sure").code, Some(1));
    }
}
