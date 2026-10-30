//! A small client for the Ollama HTTP API.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

pub const DEFAULT_HOST: &str = "http://127.0.0.1:11434";

/// `OLLAMA_HOST` forms: `host`, `host:port`, `:port`, or a URL. Bind-all addresses mean localhost.
pub fn normalize_host(raw: &str) -> String {
    let raw = raw.trim().trim_end_matches('/');
    if raw.is_empty() {
        return DEFAULT_HOST.into();
    }
    let (scheme, rest) = match raw.split_once("://") {
        Some((scheme, rest)) => (scheme.to_string(), rest.to_string()),
        None => ("http".to_string(), raw.to_string()),
    };
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority.to_string(), format!("/{path}")),
        None => (rest, String::new()),
    };
    let has_port = authority
        .rsplit_once(':')
        .is_some_and(|(_, port)| port.parse::<u16>().is_ok())
        && !authority.ends_with(']');
    let (mut hostname, port) = if has_port {
        let (host, port) = authority.rsplit_once(':').expect("port");
        (host.to_string(), port.to_string())
    } else {
        let port = if scheme == "https" { "443" } else { "11434" };
        (authority, port.to_string())
    };
    if hostname.is_empty() || hostname == "0.0.0.0" {
        hostname = "127.0.0.1".into();
    } else if hostname == "[::]" {
        hostname = "[::1]".into();
    }
    format!("{scheme}://{hostname}:{port}{path}")
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TagDetails {
    pub family: String,
    pub parameter_size: String,
    pub quantization_level: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Tag {
    pub name: String,
    pub size: u64,
    pub modified_at: String,
    pub details: TagDetails,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Show {
    pub capabilities: Vec<String>,
    pub model_info: serde_json::Map<String, Value>,
    pub details: TagDetails,
}

impl Show {
    /// `<architecture>.context_length` of `model_info`.
    pub fn context_length(&self) -> Option<u64> {
        let arch = self
            .model_info
            .get("general.architecture")
            .and_then(Value::as_str);
        let key = arch.map(|arch| format!("{arch}.context_length"));
        key.and_then(|key| self.model_info.get(&key))
            .or_else(|| {
                self.model_info
                    .iter()
                    .find(|(key, _)| key.ends_with(".context_length"))
                    .map(|(_, value)| value)
            })
            .and_then(Value::as_u64)
    }
}

/// One line of the `/api/pull` stream.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct PullProgress {
    pub status: String,
    pub digest: Option<String>,
    pub total: Option<u64>,
    pub completed: Option<u64>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct Client {
    base: String,
    http: reqwest::Client,
}

fn error(context: &str, err: impl std::fmt::Display) -> String {
    format!("{context}: {err}")
}

impl Client {
    pub fn new(base: &str) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .build()
            .unwrap_or_default();
        Self {
            base: base.trim_end_matches('/').to_string(),
            http,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    async fn check(
        response: reqwest::Response,
        context: &str,
    ) -> Result<reqwest::Response, String> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
            .unwrap_or(body);
        Err(format!("{context}: HTTP {status}: {}", message.trim()))
    }

    pub async fn tags(&self) -> Result<Vec<Tag>, String> {
        #[derive(Deserialize)]
        struct Tags {
            #[serde(default)]
            models: Vec<Tag>,
        }
        let url = format!("{}/api/tags", self.base);
        let request = self.http.get(&url).timeout(Duration::from_secs(10));
        let response = request.send().await.map_err(|e| error(&url, e))?;
        let tags: Tags = Self::check(response, &url)
            .await?
            .json()
            .await
            .map_err(|e| error(&url, e))?;
        Ok(tags.models)
    }

    pub async fn show(&self, model: &str) -> Result<Show, String> {
        let url = format!("{}/api/show", self.base);
        let request = self
            .http
            .post(&url)
            .json(&json!({ "model": model }))
            .timeout(Duration::from_secs(10));
        let response = request.send().await.map_err(|e| error(&url, e))?;
        Self::check(response, &url)
            .await?
            .json()
            .await
            .map_err(|e| error(&url, e))
    }

    pub async fn delete(&self, model: &str) -> Result<(), String> {
        let url = format!("{}/api/delete", self.base);
        let request = self
            .http
            .delete(&url)
            .json(&json!({ "model": model }))
            .timeout(Duration::from_secs(30));
        let response = request.send().await.map_err(|e| error(&url, e))?;
        Self::check(response, &url).await.map(|_| ())
    }

    /// Streams `/api/pull`; calls `progress` for each line and fails on an `error` line.
    pub async fn pull(
        &self,
        model: &str,
        mut progress: impl FnMut(&PullProgress),
    ) -> Result<(), String> {
        let url = format!("{}/api/pull", self.base);
        let request = self
            .http
            .post(&url)
            .json(&json!({ "model": model, "stream": true }));
        let response = request.send().await.map_err(|e| error(&url, e))?;
        let mut response = Self::check(response, &url).await?;
        let mut buffer = Vec::new();
        let mut handle = |line: &[u8]| -> Result<(), String> {
            if line.iter().all(u8::is_ascii_whitespace) {
                return Ok(());
            }
            let update: PullProgress =
                serde_json::from_slice(line).map_err(|e| error("pull stream", e))?;
            if let Some(message) = &update.error {
                return Err(message.clone());
            }
            progress(&update);
            Ok(())
        };
        while let Some(chunk) = response.chunk().await.map_err(|e| error(&url, e))? {
            buffer.extend_from_slice(&chunk);
            while let Some(end) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buffer.drain(..=end).collect();
                handle(&line)?;
            }
        }
        handle(&buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_ollama_host_forms() {
        assert_eq!(normalize_host(""), DEFAULT_HOST);
        assert_eq!(normalize_host("0.0.0.0"), "http://127.0.0.1:11434");
        assert_eq!(normalize_host("0.0.0.0:8080"), "http://127.0.0.1:8080");
        assert_eq!(normalize_host(":9999"), "http://127.0.0.1:9999");
        assert_eq!(normalize_host("gpu-box"), "http://gpu-box:11434");
        assert_eq!(
            normalize_host("https://ollama.example.com/"),
            "https://ollama.example.com:443"
        );
        assert_eq!(
            normalize_host("http://10.0.0.2:11434/proxy"),
            "http://10.0.0.2:11434/proxy"
        );
        assert_eq!(normalize_host("[::]:11434"), "http://[::1]:11434");
    }

    #[test]
    fn context_length_follows_the_architecture() {
        let show: Show = serde_json::from_value(json!({
            "capabilities": ["completion", "tools"],
            "model_info": { "general.architecture": "qwen2", "qwen2.context_length": 32768, "x.context_length": 1 }
        }))
        .unwrap();
        assert_eq!(show.context_length(), Some(32768));
        let other: Show =
            serde_json::from_value(json!({"model_info": {"llama.context_length": 8192}})).unwrap();
        assert_eq!(other.context_length(), Some(8192));
        assert_eq!(Show::default().context_length(), None);
    }
}
