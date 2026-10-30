//! OAuth login and refresh for extension-owned slots, ported from pi-ai
//! (`auth/oauth/openai_codex.rs`, `anthropic.rs`, `pkce.rs`, `http.rs`).

pub mod anthropic;
pub mod callback;
pub mod codex;

use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::fsutil::now_ms;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credential {
    pub access: String,
    pub refresh: String,
    /// Epoch ms.
    pub expires: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub fn base64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn pkce_challenge(verifier: &str) -> String {
    base64url(&Sha256::digest(verifier.as_bytes()))
}

/// A random 32-byte verifier (43 base64url characters) and its S256 challenge.
pub fn generate_pkce() -> Pkce {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let verifier = base64url(&bytes);
    let challenge = pkce_challenge(&verifier);
    Pkce {
        verifier,
        challenge,
    }
}

pub fn random_hex(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthorizationInput {
    pub code: Option<String>,
    pub state: Option<String>,
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query
        .trim_start_matches(['?', '#'])
        .split('&')
        .find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == key).then(|| percent_decode(v))
        })
}

pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn build_url(base: &str, params: &[(&str, &str)]) -> String {
    let query: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect();
    format!("{base}?{}", query.join("&"))
}

/// A redirect URL, `code#state`, a `code=...&state=...` query, or a bare code.
pub fn parse_authorization_input(input: &str) -> AuthorizationInput {
    let value = input.trim();
    if value.is_empty() {
        return AuthorizationInput::default();
    }
    if value.starts_with("http://") || value.starts_with("https://") {
        let query = value.split_once('?').map_or("", |(_, q)| q);
        let query = query.split('#').next().unwrap_or_default();
        return AuthorizationInput {
            code: query_param(query, "code"),
            state: query_param(query, "state"),
        };
    }
    if let Some((code, rest)) = value.split_once('#') {
        let state = rest.split('#').next().unwrap_or_default();
        return AuthorizationInput {
            code: Some(code.into()),
            state: Some(state.into()),
        };
    }
    if value.contains("code=") {
        return AuthorizationInput {
            code: query_param(value, "code"),
            state: query_param(value, "state"),
        };
    }
    AuthorizationInput {
        code: Some(value.into()),
        state: None,
    }
}

/// `access_token`, `refresh_token`, and `expires_in` of a token response.
pub fn read_token(json: &Value, margin_ms: i64, label: &str) -> Result<Credential, String> {
    let field = |k: &str| {
        json.get(k)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
    };
    let expires_in = json.get("expires_in").and_then(Value::as_f64);
    match (field("access_token"), field("refresh_token"), expires_in) {
        (Some(access), Some(refresh), Some(expires_in)) => Ok(Credential {
            access: access.into(),
            refresh: refresh.into(),
            expires: now_ms() + (expires_in * 1000.0) as i64 - margin_ms,
            account_id: None,
        }),
        _ => Err(format!("{label} token response missing fields: {json}")),
    }
}

/// POSTs a token request; non-2xx answers become errors with the body text.
pub async fn post_token(request: reqwest::RequestBuilder, label: &str) -> Result<Value, String> {
    let response = request
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|err| format!("{label} token request failed: {err}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "{label} token request failed ({}): {body}",
            status.as_u16()
        ));
    }
    serde_json::from_str(&body).map_err(|err| format!("{label} token response is not JSON: {err}"))
}
