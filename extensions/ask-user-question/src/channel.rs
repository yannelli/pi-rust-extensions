//! Open questionnaires and the inbound answer channel; the first answer wins.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pi_ext_sdk::{Host, Result};
use serde_json::{Map, Value, json};
use tokio::sync::watch;

use crate::external::{RejectCode, resolve};
use crate::result::QuestionnaireResult;
use crate::schema::Question;

pub const ANSWER_EVENT: &str = "rpiv:ask-user:answer";
pub const ANSWER_RESULT_EVENT: &str = "rpiv:ask-user:answer-result";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerHost {
    Tui,
    Rpc,
}

pub struct OpenQuestionnaire {
    pub tool_call_id: String,
    pub questions: Vec<Question>,
    host: Mutex<AnswerHost>,
    result: watch::Sender<Option<QuestionnaireResult>>,
}

impl OpenQuestionnaire {
    pub fn set_host(&self, host: AnswerHost) {
        *self.host.lock().expect("host lock") = host;
    }

    fn host(&self) -> AnswerHost {
        *self.host.lock().expect("host lock")
    }

    pub fn settled(&self) -> Option<QuestionnaireResult> {
        self.result.borrow().clone()
    }

    /// False when an earlier answer already settled this questionnaire.
    pub fn settle(&self, result: QuestionnaireResult) -> bool {
        self.result.send_if_modified(|slot| {
            if slot.is_some() {
                return false;
            }
            *slot = Some(result);
            true
        })
    }

    /// Resolves with the accepted answer.
    pub async fn answered(&self) -> QuestionnaireResult {
        let mut rx = self.result.subscribe();
        loop {
            if let Some(result) = rx.borrow_and_update().clone() {
                return result;
            }
            if rx.changed().await.is_err() {
                return std::future::pending().await;
            }
        }
    }
}

#[derive(Default)]
pub struct AnswerChannel {
    open: Mutex<HashMap<String, Arc<OpenQuestionnaire>>>,
    listening: tokio::sync::Mutex<bool>,
}

impl AnswerChannel {
    pub fn open(
        &self,
        tool_call_id: &str,
        questions: &[Question],
        host: AnswerHost,
    ) -> Arc<OpenQuestionnaire> {
        let questionnaire = Arc::new(OpenQuestionnaire {
            tool_call_id: tool_call_id.to_owned(),
            questions: questions.to_vec(),
            host: Mutex::new(host),
            result: watch::Sender::new(None),
        });
        let mut open = self.open.lock().expect("open lock");
        open.insert(tool_call_id.to_owned(), questionnaire.clone());
        questionnaire
    }

    pub fn close(&self, questionnaire: &Arc<OpenQuestionnaire>) {
        let mut open = self.open.lock().expect("open lock");
        if open
            .get(&questionnaire.tool_call_id)
            .is_some_and(|q| Arc::ptr_eq(q, questionnaire))
        {
            open.remove(&questionnaire.tool_call_id);
        }
    }

    /// Applies one inbound answer and returns its `rpiv:ask-user:answer-result` verdict.
    pub fn handle(&self, data: &Value) -> Value {
        let empty = Map::new();
        let map = data.as_object().unwrap_or(&empty);
        let request_id = map.get("requestId").and_then(Value::as_str);
        let tool_call_id = map.get("toolCallId").and_then(Value::as_str);
        let reply = |rejection: Option<(RejectCode, String)>| {
            let mut verdict = json!({ "ok": rejection.is_none() });
            if let Some((code, reason)) = rejection {
                verdict["code"] = json!(code.as_str());
                verdict["reason"] = json!(reason);
            }
            if let Some(id) = request_id {
                verdict["requestId"] = json!(id);
            }
            if let Some(id) = tool_call_id {
                verdict["toolCallId"] = json!(id);
            }
            verdict
        };
        let reject = |code, reason: &str| reply(Some((code, reason.to_owned())));
        if !data.is_object() {
            return reject(RejectCode::InvalidPayload, "payload must be an object");
        }
        if map.contains_key("requestId") && request_id.is_none() {
            return reject(RejectCode::InvalidPayload, "requestId must be a string");
        }
        let Some(id) = tool_call_id else {
            return reject(RejectCode::InvalidPayload, "toolCallId must be a string");
        };
        let target = self.open.lock().expect("open lock").get(id).cloned();
        let Some(target) = target.filter(|t| t.settled().is_none()) else {
            return reject(
                RejectCode::NotFound,
                &format!("no open questionnaire for tool call {id}"),
            );
        };
        if target.host() == AnswerHost::Rpc {
            return reject(
                RejectCode::UnsupportedHost,
                "this host shows the questions in its own dialogs",
            );
        }
        match resolve(&target.questions, map) {
            Err(rejection) => reply(Some(rejection)),
            Ok(result) => {
                if target.settle(result) {
                    reply(None)
                } else {
                    reject(
                        RejectCode::NotFound,
                        &format!("tool call {id} is already answered"),
                    )
                }
            }
        }
    }

    /// Subscribes to `rpiv:ask-user:answer` once; the listener runs until pi closes the connection.
    pub async fn ensure_listening(self: &Arc<Self>, host: &Host) -> Result<()> {
        let mut listening = self.listening.lock().await;
        if *listening {
            return Ok(());
        }
        let mut subscription = host.subscribe(ANSWER_EVENT).await?;
        *listening = true;
        let channel = self.clone();
        let host = host.clone();
        tokio::spawn(async move {
            while let Some(data) = subscription.recv().await {
                let verdict = channel.handle(&data);
                if let Err(err) = host.emit(ANSWER_RESULT_EVENT, verdict).await {
                    pi_ext_sdk::warn!("cannot emit {ANSWER_RESULT_EVENT}: {err}");
                }
            }
        });
        Ok(())
    }
}
