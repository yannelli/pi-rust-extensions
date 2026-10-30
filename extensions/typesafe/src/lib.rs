//! TypeSafe (Jev) structured judgments: the `typesafe_evaluate` tool and `/typesafe`.

use std::process::ExitCode;
use std::sync::Arc;

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::{NotifyType, paths};

pub mod auth_state;
pub mod backends;
pub mod caps;
pub mod client;
mod commands;
pub mod credentials;
pub mod errors;
#[doc(hidden)]
pub mod mock_api;
mod prompt;
pub mod result;
pub mod schema;
pub mod state;
mod status;
#[cfg(test)]
mod testutil;
pub mod usage;
pub mod validate;

use crate::auth_state::Level;
use crate::backends::BACKEND_ENV;
use crate::client::DEFAULT_MAX_REQUESTS;
use crate::credentials::Env;
use crate::errors::{Code, TsError};
use crate::state::{Project, State};

/// The request the playground starts from and the prompt guidelines quote.
pub fn sample() -> Value {
    json!({
        "state": { "message": "I was charged twice for my subscription. Please help today." },
        "questions": {
            "category": { "type": "choice", "instructions": "Which team should handle this message?", "criteria": { "billing": "Charges and payments", "technical": "Software failures", "other": "None of these" } },
            "urgent": { "type": "noul", "instructions": "Does the sender request help today?" },
            "frustration": { "type": "score", "instructions": "How frustrated does the sender sound?", "criteria": ["Neutral request", "Frustrated but civil", "Angry or threatening"] }
        }
    })
}

fn tool_spec(state: &State) -> ToolSpec {
    let label = state.backend.label;
    let (consent, _) = state.consent();
    let description = format!(
        "Evaluate supplied state with independent Choice, Score, and Noul questions in one {label} request. Each question judges the whole state, so when several items are involved, put each item in a named state field (e.g. `reports.r1`) and ask one question per item per dimension (e.g. `r1_owner`, `r2_owner`), naming the field in the instructions; never aggregate several items into one question. {consent} Requires operator opt-in via /typesafe enable or PI_TYPESAFE_ENABLED=1. Limit: 32 questions, 64 KiB JSON, {DEFAULT_MAX_REQUESTS} attempts per session; no retries."
    );
    ToolSpec::new("typesafe_evaluate", description)
        .label("TypeSafe")
        .parameters(schema::evaluation_schema())
        .prompt_snippet(format!("Ask batched structured questions with {label} (external service; operator opt-in required)"))
        .guideline(format!("Request shape, all three question kinds in one call: {}", sample()))
        .guideline("Use typesafe_evaluate only for requested semantic judgments, not calculations or exact lookups; send only the relevant permitted data.")
        .guideline("Batch independent typesafe_evaluate questions over the same state; use code or explicit permission rules for actions, never confidence as authorization.")
        .guideline("When typesafe_evaluate judges several items, give each item a named state field and ask one question per item per dimension, naming the field in the instructions; one question over many items returns an unusable blend.")
        .guideline("Report typesafe_evaluate answers as the model's judgments with their probabilities; do not replace them with your own guesses, and say when an answer is uncertain.")
}

async fn call_out_auth(state: &State, host: &Host, error: &TsError) {
    if error.is_rejected_key() || error.code == Code::Configuration {
        let key = format!(
            "run:{}:{}",
            error.code.as_str(),
            error.status.map_or(String::new(), |s| s.to_string())
        );
        let text = format!(
            "TypeSafe is not authenticated ({}) Judgments will fail until the key is fixed.",
            error.message
        );
        state.call_out(host, &key, &text).await;
    }
}

async fn evaluate(state: Arc<State>, args: Value, ctx: ToolCtx) -> Result<ToolResult> {
    if !state.enabled() {
        bail!(
            "TypeSafe is disabled. Ask the operator to run /typesafe enable; do not enable it by editing configuration or environment files."
        );
    }
    if let Some(error) = &state.backend_error {
        return Err(Error::msg(error));
    }
    let request = validate::prepare(args, None)?;
    let count = request["questions"].as_object().map_or(0, |q| q.len());
    let _ = ctx.update(format!("TypeSafe · {count} questions · external request"));
    let outcome = match state.get_client() {
        Ok(client) => client.evaluate(request, ctx.cancel_token()).await,
        Err(error) => Err(error),
    };
    match outcome {
        Ok(result) => Ok(ToolResult::text(result.to_string()).details(result)),
        Err(error) => {
            call_out_auth(&state, ctx.host(), &error).await;
            Err(error.into())
        }
    }
}

async fn session_start(state: Arc<State>, ev: EventCtx) -> Result<()> {
    let host = ev.host().clone();
    let snapshot = ev.snapshot();
    let trusted = snapshot
        .extra
        .get("isProjectTrusted")
        .and_then(Value::as_bool)
        == Some(true);
    let project = Project::load(&snapshot.cwd_path(), trusted);
    if project.ignored_enable() && !state.project_notice_shown() {
        let text = "TypeSafe ignores \"enabled\" in .pi/ext/typesafe.json because this project is not trusted.";
        notify(&host, text, NotifyType::Info).await;
    }
    state.reset_session(&project);
    let (level, text) = state.describe_auth();
    if let Some(error) = &state.backend_error {
        let text = format!(
            "{error} Judgments are skipped until {BACKEND_ENV} names a valid backend or is unset."
        );
        state.call_out(&host, "start:backend", &text).await;
    } else if state.enabled() && level == Level::Error {
        let text = format!("TypeSafe is enabled but judgments are skipped. {text}");
        state.call_out(&host, "start:error", &text).await;
    }
    Ok(())
}

/// The extension over a given state; tests pass a fixed environment and agent dir.
pub fn extension_with(state: Arc<State>) -> Extension {
    let (tool, start, command, complete) =
        (state.clone(), state.clone(), state.clone(), state.clone());
    Extension::new("typesafe")
        .tool(tool_spec(&state), move |args: Value, ctx: ToolCtx| {
            evaluate(tool.clone(), args, ctx)
        })
        .on("session_start", move |ev: EventCtx| {
            let state = start.clone();
            async move { session_start(state, ev).await }
        })
        .command(
            "typesafe",
            "TypeSafe login, consent, usage, sample test, and JSON playground",
            move |args, ctx: CommandCtx| commands::run(command.clone(), args, ctx),
        )
        .completions("typesafe", move |prefix, _host| {
            let _ = &complete;
            async move { Ok(commands::complete(&prefix)) }
        })
}

pub fn extension() -> Extension {
    extension_with(State::new(Env::process(), &paths::agent_dir()))
}

pub fn main() -> ExitCode {
    extension().run()
}

pub(crate) async fn notify(host: &Host, text: &str, level: NotifyType) {
    state::report(host, text, level).await;
}
