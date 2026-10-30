//! OpenAI Responses `web_search` tool: OpenAI, Azure, Codex, Copilot, xAI, and OpenCode.
//! Docs: https://platform.openai.com/docs/guides/tools-web-search

use anyhow::{Result, anyhow, bail};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};

use super::openai_events::Collector;
use crate::http::Call;
use crate::model::{Kind, Target};
use crate::results::StreamResult;

pub fn responses_url(target: &Target) -> String {
    let base = copilot_base_url(target);
    let base = base.trim_end_matches('/');
    if !target.model.is_codex() {
        return format!("{base}/responses");
    }
    if base.ends_with("/codex/responses") {
        base.to_owned()
    } else if base.ends_with("/codex") {
        format!("{base}/responses")
    } else {
        format!("{base}/codex/responses")
    }
}

fn valid_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    let edge = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    !label.is_empty()
        && label.len() <= 63
        && edge(bytes[0])
        && edge(bytes[bytes.len() - 1])
        && bytes.iter().all(|b| edge(*b) || *b == b'-')
}

/// Copilot tokens carry `proxy-ep=proxy.<host>`; the API lives at `api.<host>`.
pub fn copilot_base_url(target: &Target) -> String {
    let model = &target.model;
    if model.provider != "github-copilot" {
        return model.base_url.clone();
    }
    let Some(key) = &target.auth.api_key else {
        return model.base_url.clone();
    };
    let endpoints: Vec<&str> = key
        .split(';')
        .filter_map(|f| f.strip_prefix("proxy-ep="))
        .collect();
    let [endpoint] = endpoints.as_slice() else {
        return model.base_url.clone();
    };
    let host = endpoint.to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').collect();
    let n = labels.len();
    let valid = host.len() <= 253
        && n >= 4
        && labels[0] == "proxy"
        && labels[n - 2] == "githubcopilot"
        && labels[n - 1] == "com"
        && labels.iter().all(|l| valid_label(l));
    if !valid {
        return model.base_url.clone();
    }
    format!("https://api.{}", labels[1..].join("."))
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in input.trim_end_matches('=').bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The `chatgpt_account_id` claim of a Codex OAuth JWT.
pub fn codex_account_id(token: &str) -> Result<String> {
    let failed = || anyhow!("Failed to extract ChatGPT account ID from openai-codex credentials");
    let parts: Vec<&str> = token.split('.').collect();
    let [_, payload, _] = parts.as_slice() else {
        return Err(failed());
    };
    let payload: Value = serde_json::from_slice(&base64url_decode(payload).ok_or_else(failed)?)?;
    let id = payload["https://api.openai.com/auth"]["chatgpt_account_id"].as_str();
    id.filter(|id| !id.is_empty())
        .map(str::to_owned)
        .ok_or_else(failed)
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
    let pairs = target.session_headers().into_iter();
    let pairs = pairs
        .chain(target.model.headers.clone())
        .chain(target.auth.headers.clone());
    for (name, value) in pairs {
        set(&mut headers, &name, &value)?;
    }
    for (name, value) in [
        ("content-type", "application/json"),
        ("accept", "text/event-stream"),
    ] {
        if !headers.contains_key(name) {
            set(&mut headers, name, value)?;
        }
    }
    if let Some(key) = &target.auth.api_key {
        if !headers.contains_key("authorization") {
            set(&mut headers, "authorization", &format!("Bearer {key}"))?;
        }
    }
    if target.model.is_codex() {
        let bearer = headers.get("authorization").and_then(|v| v.to_str().ok());
        let has_bearer =
            bearer.is_some_and(|v| v.len() > 7 && v[..7].eq_ignore_ascii_case("bearer "));
        if target.auth.api_key.is_none() && !has_bearer {
            bail!("No OAuth credential configured for openai-codex model");
        }
        if !headers.contains_key("chatgpt-account-id") {
            let Some(key) = &target.auth.api_key else {
                bail!("No ChatGPT account ID configured for openai-codex model");
            };
            set(&mut headers, "chatgpt-account-id", &codex_account_id(key)?)?;
        }
        if !headers.contains_key("originator") {
            set(&mut headers, "originator", "codex_cli_rs")?;
        }
    }
    Ok(headers)
}

pub fn build_body(target: &Target, prompt: &str) -> Value {
    let model = &target.model;
    let (codex, grok) = (model.is_codex(), model.is_grok_dialect());
    let input = if grok && !codex {
        json!([{ "role": "user", "content": prompt }])
    } else {
        json!([{ "role": "user", "content": [{ "type": "input_text", "text": prompt }] }])
    };
    let include = if codex || grok {
        json!(["web_search_call.action.sources"])
    } else {
        json!(["web_search_call.action.sources", "web_search_call.results"])
    };
    let mut body = json!({
        "model": model.id, "input": input, "tools": [{ "type": "web_search" }],
        "include": include, "stream": true, "store": false,
    });
    if model.kind() != Kind::Xai {
        if let Some(effort) = model.reasoning_effort(target.thinking_level.as_deref()) {
            body["reasoning"] = json!({ "effort": effort });
        }
    }
    if codex {
        body["instructions"] = json!("Answer the user's request using web search when needed.");
        body["text"] = json!({ "verbosity": "low" });
        body["tool_choice"] = json!("required");
        body["parallel_tool_calls"] = json!(true);
    }
    body
}

pub async fn search(call: &Call, target: &Target, prompt: &str) -> Result<StreamResult> {
    let xai = target.model.kind() == Kind::Xai;
    let request = call
        .client
        .post(responses_url(target))
        .headers(build_headers(target)?)
        .json(&build_body(target, prompt));
    let response = call
        .send(request, if xai { "xAI" } else { "OpenAI" })
        .await?;
    let mut collector = Collector::new(xai, target.model.is_codex());
    call.sse(response, |event| collector.handle(&event.data, call))
        .await?;
    Ok(collector.finish(target.model.is_grok_dialect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Auth, SearchModel};

    fn target(provider: &str, api: &str, key: &str) -> Target {
        let raw = json!({ "id": "gpt", "provider": provider, "api": api, "baseUrl": "https://x.test/v1" });
        Target {
            model: SearchModel::from_json(raw).unwrap(),
            auth: Auth {
                api_key: Some(key.into()),
                ..Auth::default()
            },
            ..Target::default()
        }
    }

    #[test]
    fn codex_url_and_account_header() {
        let payload = r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct"}}"#;
        let encoded: String = payload.bytes().map(|b| b as char).collect();
        let token = format!("h.{}.s", base64url_encode(encoded.as_bytes()));
        let t = target("openai-codex", "openai-codex-responses", &token);
        assert_eq!(responses_url(&t), "https://x.test/v1/codex/responses");
        let headers = build_headers(&t).unwrap();
        assert_eq!(headers["chatgpt-account-id"], "acct");
        assert_eq!(headers["originator"], "codex_cli_rs");
        assert_eq!(build_body(&t, "q")["tool_choice"], "required");
    }

    #[test]
    fn copilot_proxy_endpoint() {
        let t = target(
            "github-copilot",
            "openai-responses",
            "tid=1;proxy-ep=proxy.enterprise.githubcopilot.com;x=y",
        );
        assert_eq!(
            copilot_base_url(&t),
            "https://api.enterprise.githubcopilot.com"
        );
        let bad = target("github-copilot", "openai-responses", "proxy-ep=evil.com");
        assert_eq!(copilot_base_url(&bad), "https://x.test/v1");
    }

    fn base64url_encode(bytes: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
            for i in 0..=chunk.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            }
        }
        out
    }
}
