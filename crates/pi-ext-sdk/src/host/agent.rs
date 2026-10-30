use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{Host, without_nulls};
use crate::bus::Subscription;
use crate::error::Result;
use crate::snapshot::Model;

/// Params of `llm.complete`. Build with `LlmRequest::prompt` or `LlmRequest::messages`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Value>,
    /// Request headers over pi's; a header with the same name replaces pi's.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

impl LlmRequest {
    pub fn prompt(prompt: impl Into<String>) -> Self {
        Self {
            prompt: Some(prompt.into()),
            ..Self::default()
        }
    }

    /// pi-ai messages.
    pub fn messages(messages: Vec<Value>) -> Self {
        Self {
            messages: Some(messages),
            ..Self::default()
        }
    }

    /// `provider/id`; the current model when unset.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn system(mut self, system_prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }

    pub fn max_tokens(mut self, max_tokens: u64) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    pub fn temperature(mut self, temperature: f64) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn reasoning(mut self, reasoning: impl Into<Value>) -> Self {
        self.reasoning = Some(reasoning.into());
        self
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct LlmResponse {
    pub message: Value,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderAuth {
    pub api_key: Option<String>,
    pub headers: Map<String, Value>,
    pub source: Option<String>,
}

/// Params of `exec`. `timeout` is in milliseconds.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ExecRequest {
    pub command: String,
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    /// Written to the process's standard input, which then closes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdin: Option<String>,
}

impl ExecRequest {
    pub fn new<S: AsRef<str>>(command: impl Into<String>, args: &[S]) -> Self {
        Self {
            command: command.into(),
            args: args.iter().map(|arg| arg.as_ref().to_owned()).collect(),
            ..Self::default()
        }
    }

    pub fn cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn timeout_ms(mut self, timeout: u64) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn stdin(mut self, stdin: impl Into<String>) -> Self {
        self.stdin = Some(stdin.into());
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i64>,
    pub killed: bool,
}

impl ExecOutput {
    pub fn success(&self) -> bool {
        self.code == Some(0) && !self.killed
    }
}

/// `ctx.*`, `llm.complete`, `auth.getProviderAuth`, `events.*`, `exec`, `provider.*`.
impl Host {
    /// Compacts the session and returns the compaction result when it completes.
    pub async fn compact(&self, custom_instructions: Option<&str>) -> Result<Value> {
        let params = without_nulls(json!({ "customInstructions": custom_instructions }));
        self.call("ctx.compact", params).await
    }

    pub async fn abort(&self) -> Result<()> {
        self.call("ctx.abort", json!({})).await.map(drop)
    }

    pub async fn is_idle(&self) -> Result<bool> {
        let idle: Option<bool> = self.call_as("ctx.isIdle", json!({})).await?;
        Ok(idle.unwrap_or(true))
    }

    /// A model of pi's registry in the snapshot shape; `None` when pi does not know it.
    pub async fn find_model(&self, provider: &str, id: &str) -> Result<Option<Model>> {
        self.call_as("model.find", json!({ "provider": provider, "id": id }))
            .await
    }

    pub async fn context_usage(&self) -> Result<Option<Value>> {
        self.call_as("ctx.getContextUsage", json!({})).await
    }

    pub async fn system_prompt(&self) -> Result<String> {
        let prompt: Option<String> = self.call_as("ctx.getSystemPrompt", json!({})).await?;
        Ok(prompt.unwrap_or_default())
    }

    /// One-shot completion with pi's credentials.
    pub async fn complete(&self, request: LlmRequest) -> Result<LlmResponse> {
        self.call_as("llm.complete", serde_json::to_value(request)?)
            .await
    }

    pub async fn provider_auth(&self, provider: &str) -> Result<Option<ProviderAuth>> {
        let params = json!({ "provider": provider });
        self.call_as("auth.getProviderAuth", params).await
    }

    /// Emits `data` on the `pi.events` bus.
    pub async fn emit(&self, channel: &str, data: impl Serialize) -> Result<()> {
        let params = json!({ "channel": channel, "data": serde_json::to_value(data)? });
        self.call("events.emit", params).await.map(drop)
    }

    /// Receives `pi.events` messages on `channel` until the subscription drops.
    pub async fn subscribe(&self, channel: &str) -> Result<Subscription> {
        let (subscription, first) = self.inner.subscriptions.add(self.clone(), channel);
        if first {
            self.call("events.subscribe", json!({ "channel": channel }))
                .await?;
        }
        Ok(subscription)
    }

    /// Runs a process through pi.
    pub async fn exec<S: AsRef<str>>(&self, command: &str, args: &[S]) -> Result<ExecOutput> {
        self.exec_with(ExecRequest::new(command, args)).await
    }

    pub async fn exec_with(&self, request: ExecRequest) -> Result<ExecOutput> {
        self.call_as("exec", serde_json::to_value(request)?).await
    }

    /// Registers the declarative part of a provider (`baseUrl`, `api`, `apiKey`, `headers`, `models`, `oauth`).
    pub async fn register_provider(&self, name: &str, config: Value) -> Result<()> {
        let params = json!({ "name": name, "config": config });
        self.call("provider.register", params).await.map(drop)
    }

    pub async fn unregister_provider(&self, name: &str) -> Result<()> {
        let params = json!({ "name": name });
        self.call("provider.unregister", params).await.map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_stdin_is_optional() {
        let plain = serde_json::to_value(ExecRequest::new("cat", &[] as &[&str])).unwrap();
        assert_eq!(plain, json!({ "command": "cat", "args": [] }));
        let piped = serde_json::to_value(ExecRequest::new("cat", &["-"]).stdin("text")).unwrap();
        assert_eq!(
            piped,
            json!({ "command": "cat", "args": ["-"], "stdin": "text" })
        );
    }

    #[test]
    fn llm_headers_are_optional() {
        let plain = serde_json::to_value(LlmRequest::prompt("hi")).unwrap();
        assert_eq!(plain, json!({ "prompt": "hi" }));
        let request = LlmRequest::prompt("hi")
            .header("x-trace", "1")
            .header("x-a", "b");
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["headers"], json!({ "x-a": "b", "x-trace": "1" }));
    }
}
