//! HTTP calls with cancellation, progress updates, and a server-sent events reader.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use pi_ext_sdk::CancellationToken;
use reqwest::{RequestBuilder, Response};
use serde::de::DeserializeOwned;
use serde_json::Value;

pub type Progress = Arc<dyn Fn(&str) + Send + Sync>;

/// One tool call: the HTTP client, the abort token, and the progress sink.
#[derive(Clone)]
pub struct Call {
    pub client: reqwest::Client,
    pub cancel: CancellationToken,
    progress: Progress,
}

impl Call {
    pub fn new(cancel: CancellationToken, progress: Progress) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self {
            client,
            cancel,
            progress,
        }
    }

    pub fn quiet() -> Self {
        Self::new(CancellationToken::new(), Arc::new(|_| {}))
    }

    pub fn progress(&self, text: &str) {
        (self.progress)(text);
    }

    async fn cancellable<T>(&self, work: impl Future<Output = T>) -> Result<T> {
        tokio::select! {
            _ = self.cancel.cancelled() => bail!("Request was aborted"),
            value = work => Ok(value),
        }
    }

    /// Sends the request; a status outside 2xx fails with `<label> API error (<status>): <body>`.
    pub async fn send(&self, request: RequestBuilder, label: &str) -> Result<Response> {
        let response = self.cancellable(request.send()).await??;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = self.cancellable(response.text()).await?.unwrap_or_default();
        Err(anyhow!(ApiError {
            status: status.as_u16(),
            message: format!("{label} API error ({}): {body}", status.as_u16()),
        }))
    }

    pub async fn json<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
        label: &str,
    ) -> Result<T> {
        let response = self.send(request, label).await?;
        let bytes = self.cancellable(response.bytes()).await??;
        serde_json::from_slice(&bytes)
            .map_err(|err| anyhow!("{label} returned invalid JSON: {err}"))
    }

    /// Calls `on_event` for each `data:` JSON event until it returns `true` or the stream ends.
    pub async fn sse(
        &self,
        mut response: Response,
        mut on_event: impl FnMut(SseEvent) -> Result<bool>,
    ) -> Result<()> {
        let mut parser = SseParser::default();
        while let Some(chunk) = self.cancellable(response.chunk()).await?? {
            for event in parser.feed(&chunk) {
                if on_event(event)? {
                    return Ok(());
                }
            }
        }
        if let Some(event) = parser.finish() {
            on_event(event)?;
        }
        Ok(())
    }
}

/// A non-2xx answer; `status` lets callers word known failures.
#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

#[derive(Debug, Clone, PartialEq)]
pub struct SseEvent {
    pub event: String,
    pub data: Value,
}

#[derive(Default)]
pub struct SseParser {
    buffer: Vec<u8>,
    data: String,
    event: String,
}

impl SseParser {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line[..end]);
            let line = line.strip_suffix('\r').unwrap_or(&line);
            if line.is_empty() {
                events.extend(self.flush());
            } else {
                self.line(line);
            }
        }
        events
    }

    pub fn finish(&mut self) -> Option<SseEvent> {
        let rest = std::mem::take(&mut self.buffer);
        let rest = String::from_utf8_lossy(&rest);
        if !rest.trim().is_empty() {
            self.line(rest.trim());
        }
        self.flush()
    }

    fn line(&mut self, line: &str) {
        if let Some(data) = line.strip_prefix("data:") {
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(data.trim());
        } else if let Some(name) = line.strip_prefix("event:") {
            self.event = name.trim().to_owned();
        }
    }

    fn flush(&mut self) -> Option<SseEvent> {
        let raw = std::mem::take(&mut self.data);
        let event = std::mem::take(&mut self.event);
        let raw = raw.trim();
        if raw.is_empty() || raw == "[DONE]" {
            return None;
        }
        let data = serde_json::from_str(raw).ok()?;
        Some(SseEvent { event, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_events_split_across_chunks() {
        let mut parser = SseParser::default();
        assert!(parser.feed(b"event: a\ndata: {\"x\":").is_empty());
        let events = parser.feed(b"1}\r\n\r\ndata: [DONE]\n\ndata: {\"y\":2}");
        assert_eq!(
            events,
            vec![SseEvent {
                event: "a".into(),
                data: json!({ "x": 1 })
            }]
        );
        assert_eq!(parser.finish().unwrap().data, json!({ "y": 2 }));
    }

    #[test]
    fn skips_invalid_json_and_joins_multiline_data() {
        let mut parser = SseParser::default();
        let events = parser.feed(b"data: nope\n\ndata: {\"a\":\ndata: 1}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, json!({ "a": 1 }));
    }
}
