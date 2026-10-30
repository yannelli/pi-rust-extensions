//! `ask_user_question` execution: validation, events, panel or dialog walker, envelope.

use std::sync::Arc;

use pi_ext_sdk::{Host, Result, ToolCtx, ToolResult};
use serde_json::{Value, json};

use crate::channel::{AnswerChannel, AnswerHost, OpenQuestionnaire};
use crate::config::Config;
use crate::fallback::walk;
use crate::guidance::{ERROR_NO_CUSTOM_UI, ERROR_NO_UI};
use crate::panel::run_panel;
use crate::result::{build_response, error_result};
use crate::schema::{QuestionParams, QuestionnaireError, normalize, prepare_arguments, validate};
use crate::state::Questionnaire;

pub const PROMPT_EVENT: &str = "rpiv:ask-user:prompt";
pub const BLOCKED_EVENT: &str = "rpiv:ask-user:blocked";
pub const BEL: &str = "\x07";

pub fn prompt_payload(params: &QuestionParams, tool_call_id: &str) -> Value {
    let questions: Vec<Value> = params
        .questions
        .iter()
        .map(|q| {
            let options: Vec<Value> = q
                .options
                .iter()
                .map(|o| json!({ "label": o.label, "description": o.description, "hasPreview": o.preview().is_some() }))
                .collect();
            json!({ "question": q.question, "header": q.header, "multiSelect": q.is_multi(), "options": options })
        })
        .collect();
    json!({ "toolCallId": tool_call_id, "questions": questions })
}

async fn emit(host: &Host, channel: &str, data: Value) {
    if let Err(err) = host.emit(channel, data).await {
        pi_ext_sdk::warn!("cannot emit {channel}: {}", err.message());
    }
}

async fn blocked(host: &Host, active: bool, tool_call_id: &str) {
    emit(
        host,
        BLOCKED_EVENT,
        json!({ "active": active, "toolCallId": tool_call_id }),
    )
    .await;
}

async fn bell(host: &Host) {
    let _ = host.write_terminal(BEL, false).await;
}

pub async fn execute(args: Value, ctx: ToolCtx, channel: Arc<AnswerChannel>) -> Result<ToolResult> {
    let params: QuestionParams = match serde_json::from_value(prepare_arguments(args)) {
        Ok(params) => normalize(params),
        Err(err) => {
            return Ok(ToolResult::error(format!(
                "Error: invalid parameters: {err}"
            )));
        }
    };
    if !ctx.snapshot().has_ui {
        return Ok(error_result(ERROR_NO_UI, QuestionnaireError::NoUi));
    }
    if let Err((error, message)) = validate(&params) {
        return Ok(error_result(message, error));
    }
    let host = ctx.host().clone();
    if let Err(err) = channel.ensure_listening(&host).await {
        pi_ext_sdk::warn!("cannot subscribe to answers: {}", err.message());
    }
    let via_rpc = ctx.snapshot().mode.as_deref() == Some("rpc");
    let answer_host = if via_rpc {
        AnswerHost::Rpc
    } else {
        AnswerHost::Tui
    };
    let id = ctx.tool_call_id().to_owned();
    let open = channel.open(&id, &params.questions, answer_host);
    emit(&host, PROMPT_EVENT, prompt_payload(&params, &id)).await;
    blocked(&host, true, &id).await;
    let result = if via_rpc {
        bell(&host).await;
        walk_dialogs(&host, &params).await
    } else {
        tui_path(&host, &ctx, &open, &params).await
    };
    blocked(&host, false, &id).await;
    channel.close(&open);
    result
}

async fn walk_dialogs(host: &Host, params: &QuestionParams) -> Result<ToolResult> {
    match walk(host, &params.questions).await {
        Ok(result) => Ok(build_response(Some(result), params)),
        Err(err) if err.is_no_ui() => Ok(error_result(
            ERROR_NO_CUSTOM_UI,
            QuestionnaireError::NoCustomUi,
        )),
        Err(err) => Err(err),
    }
}

async fn tui_path(
    host: &Host,
    ctx: &ToolCtx,
    open: &OpenQuestionnaire,
    params: &QuestionParams,
) -> Result<ToolResult> {
    if let Some(result) = open.settled() {
        return Ok(build_response(Some(result), params));
    }
    let config = Config::load(&host.agent_dir(), &host.cwd());
    let state = Questionnaire::new(params.questions.clone())
        .with_keys(&config.collapse_key, &config.notes_key);
    bell(host).await;
    match run_panel(host, ctx.cancel_token(), open, state).await? {
        Some(result) => Ok(build_response(Some(result), params)),
        None => {
            open.set_host(AnswerHost::Rpc);
            walk_dialogs(host, params).await
        }
    }
}
