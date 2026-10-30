//! Anthropic Messages server tool `web_search_20260209`, also used by DeepSeek.
//! Docs: https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/web-search-tool

use anyhow::{Result, bail};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};

use super::anthropic_events::State;
use crate::http::Call;
use crate::model::{Kind, Target};
use crate::results::StreamResult;

const CLAUDE_CODE_SYSTEM_PROMPT: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

pub fn messages_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/messages")
    } else {
        format!("{base}/v1/messages")
    }
}

/// DeepSeek serves search only on `/anthropic`; a proxy keeps its own origin and path.
pub fn deepseek_base_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with("/anthropic") || base.ends_with("/anthropic/v1") {
        return base.to_owned();
    }
    format!("{}/anthropic", base.strip_suffix("/v1").unwrap_or(base))
}

fn is_oauth(target: &Target) -> bool {
    let key = target.auth.api_key.as_deref().unwrap_or_default();
    target.model.kind() != Kind::DeepSeek && key.contains("sk-ant-oat")
}

fn set(headers: &mut HeaderMap, name: &str, value: &str) -> Result<()> {
    headers.insert(
        HeaderName::from_bytes(name.as_bytes())?,
        HeaderValue::from_str(value)?,
    );
    Ok(())
}

pub fn build_headers(target: &Target) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    let defaults = [
        ("content-type", "application/json"),
        ("accept", "text/event-stream"),
        ("anthropic-version", "2023-06-01"),
    ];
    let defaults = defaults
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()));
    let pairs = defaults.chain(target.session_headers());
    let pairs = pairs
        .chain(target.model.headers.clone())
        .chain(target.auth.headers.clone());
    for (name, value) in pairs {
        set(&mut headers, &name, &value)?;
    }
    if let Some(key) = &target.auth.api_key {
        if is_oauth(target) {
            if !headers.contains_key("authorization") {
                set(&mut headers, "authorization", &format!("Bearer {key}"))?;
            }
            let beta = "claude-code-20250219,oauth-2025-04-20";
            let beta = match headers.get("anthropic-beta").and_then(|v| v.to_str().ok()) {
                Some(existing) => format!("{existing},{beta}"),
                None => beta.to_owned(),
            };
            set(&mut headers, "anthropic-beta", &beta)?;
            for (name, value) in [("user-agent", "claude-cli/2.1.251"), ("x-app", "cli")] {
                if !headers.contains_key(name) {
                    set(&mut headers, name, value)?;
                }
            }
        } else if !headers.contains_key("x-api-key") {
            set(&mut headers, "x-api-key", key)?;
        }
    }
    let has_key = headers.contains_key("authorization") || headers.contains_key("x-api-key");
    if target.model.kind() == Kind::DeepSeek && !has_key {
        bail!(
            "No DeepSeek API key found. Run /login in pi and select DeepSeek, or set DEEPSEEK_API_KEY."
        );
    }
    Ok(headers)
}

pub fn build_body(target: &Target, prompt: &str) -> Value {
    let third = target.model.max_tokens / 3;
    let max_tokens = if third == 0 { 4096 } else { third.max(1024) }.min(8192);
    let mut body = json!({
        "model": target.model.id,
        "max_tokens": max_tokens,
        "messages": [{ "role": "user", "content": prompt }],
        "tools": [{
            "type": "web_search_20260209", "name": "web_search", "max_uses": 10,
            "allowed_callers": ["direct"]
        }],
        "stream": true,
    });
    if is_oauth(target) {
        body["system"] = json!([{ "type": "text", "text": CLAUDE_CODE_SYSTEM_PROMPT }]);
    }
    body
}

pub async fn search(call: &Call, target: &Target, prompt: &str) -> Result<StreamResult> {
    let deepseek = target.model.kind() == Kind::DeepSeek;
    let (kind, label) = if deepseek {
        ("deepseek", "DeepSeek")
    } else {
        ("anthropic", "Anthropic")
    };
    let base = &target.model.base_url;
    let url = messages_url(&if deepseek {
        deepseek_base_url(base)
    } else {
        base.clone()
    });
    let request = call
        .client
        .post(url)
        .headers(build_headers(target)?)
        .json(&build_body(target, prompt));
    let response = call.send(request, label).await?;
    let mut state = State::new(kind, label, deepseek);
    call.sse(response, |event| {
        state.handle(&event.data, call).map(|()| false)
    })
    .await?;
    Ok(state.finish())
}
