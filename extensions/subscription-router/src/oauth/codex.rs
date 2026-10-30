//! ChatGPT (OpenAI Codex) OAuth: PKCE browser login with a localhost:1455 callback.

use super::{Credential, build_url, post_token, read_token};
use crate::usage::codex_account_id;

pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
pub const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
pub const CALLBACK_PORT: u16 = 1455;
pub const CALLBACK_PATH: &str = "/auth/callback";
pub const SCOPE: &str = "openid profile email offline_access";

pub fn token_url() -> String {
    std::env::var("PI_ROUTER_CODEX_TOKEN_URL").unwrap_or_else(|_| TOKEN_URL.into())
}

pub fn authorize_url(challenge: &str, state: &str, originator: &str) -> String {
    build_url(
        AUTHORIZE_URL,
        &[
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", REDIRECT_URI),
            ("scope", SCOPE),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("state", state),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("originator", originator),
        ],
    )
}

fn credential(json: &serde_json::Value, operation: &str) -> Result<Credential, String> {
    let mut credential = read_token(json, 0, &format!("OpenAI Codex {operation}"))?;
    let account = codex_account_id(&credential.access);
    credential.account_id = Some(account.ok_or("Failed to extract accountId from token")?);
    Ok(credential)
}

pub async fn exchange(
    client: &reqwest::Client,
    url: &str,
    code: &str,
    verifier: &str,
) -> Result<Credential, String> {
    let form = [
        ("grant_type", "authorization_code"),
        ("client_id", CLIENT_ID),
        ("code", code),
        ("code_verifier", verifier),
        ("redirect_uri", REDIRECT_URI),
    ];
    let json = post_token(client.post(url).form(&form), "OpenAI Codex").await?;
    credential(&json, "exchange")
}

pub async fn refresh(
    client: &reqwest::Client,
    url: &str,
    refresh_token: &str,
) -> Result<Credential, String> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", CLIENT_ID),
    ];
    let json = post_token(client.post(url).form(&form), "OpenAI Codex").await?;
    credential(&json, "refresh")
}
