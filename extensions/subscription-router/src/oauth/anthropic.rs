//! Anthropic (Claude Pro/Max) OAuth: PKCE login where the user pastes the `code#state` Anthropic shows.

use base64::Engine;
use serde_json::json;

use super::{Credential, build_url, post_token, read_token};

const CLIENT_ID_ENCODED: &str = "OWQxYzI1MGEtZTYxYi00NGQ5LTg4ZWQtNTk0NGQxOTYyZjVl";
pub const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
pub const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
pub const REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
pub const SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
const EXPIRY_MARGIN_MS: i64 = 5 * 60_000;

pub fn client_id() -> String {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(CLIENT_ID_ENCODED)
        .unwrap_or_default();
    String::from_utf8(bytes).unwrap_or_default()
}

pub fn token_url() -> String {
    std::env::var("PI_ROUTER_ANTHROPIC_TOKEN_URL").unwrap_or_else(|_| TOKEN_URL.into())
}

/// pi-ai uses the PKCE verifier as the OAuth `state`.
pub fn authorize_url(challenge: &str, state: &str) -> String {
    let client_id = client_id();
    build_url(
        AUTHORIZE_URL,
        &[
            ("code", "true"),
            ("client_id", &client_id),
            ("response_type", "code"),
            ("redirect_uri", REDIRECT_URI),
            ("scope", SCOPES),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("state", state),
        ],
    )
}

pub async fn exchange(
    client: &reqwest::Client,
    url: &str,
    code: &str,
    state: &str,
    verifier: &str,
) -> Result<Credential, String> {
    let body = json!({
        "grant_type": "authorization_code",
        "client_id": client_id(),
        "code": code,
        "state": state,
        "redirect_uri": REDIRECT_URI,
        "code_verifier": verifier,
    });
    let request = client
        .post(url)
        .header("Accept", "application/json")
        .json(&body);
    let json = post_token(request, "Anthropic").await?;
    read_token(&json, EXPIRY_MARGIN_MS, "Anthropic")
}

pub async fn refresh(
    client: &reqwest::Client,
    url: &str,
    refresh_token: &str,
) -> Result<Credential, String> {
    let body = json!({
        "grant_type": "refresh_token",
        "client_id": client_id(),
        "refresh_token": refresh_token,
    });
    let request = client
        .post(url)
        .header("Accept", "application/json")
        .json(&body);
    let json = post_token(request, "Anthropic").await?;
    read_token(&json, EXPIRY_MARGIN_MS, "Anthropic")
}
