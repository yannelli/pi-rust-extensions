//! Interactive OAuth login of an extension slot through `ui.notify`, `ui.input`, and `exec`.

use pi_ext_sdk::Host;

use crate::accounts::Account;
use crate::family::Family;
use crate::oauth::{self, anthropic, callback, codex, parse_authorization_input};
use crate::providers;
use crate::state::State;

/// Best-effort browser launch; the URL is also shown with `ui.notify`.
pub async fn open_browser(host: &Host, url: &str) {
    let (command, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(windows) {
        ("rundll32", vec!["url.dll,FileProtocolHandler", url])
    } else {
        ("xdg-open", vec![url])
    };
    if let Err(err) = host.exec(command, &args).await {
        pi_ext_sdk::debug!("cannot open a browser: {err}");
    }
}

/// The code of pasted input; a state that does not match fails.
pub fn pasted_code(
    input: Option<String>,
    expected_state: &str,
) -> Result<(String, Option<String>), String> {
    let input = input.ok_or("Login cancelled")?;
    let parsed = parse_authorization_input(&input);
    if parsed
        .state
        .as_deref()
        .is_some_and(|s| !s.is_empty() && s != expected_state)
    {
        return Err("State mismatch".into());
    }
    let code = parsed
        .code
        .filter(|c| !c.is_empty())
        .ok_or("Missing authorization code")?;
    Ok((code, parsed.state.filter(|s| !s.is_empty())))
}

async fn codex_login(host: &Host, state: &State, name: &str) -> Result<oauth::Credential, String> {
    let pkce = oauth::generate_pkce();
    let oauth_state = oauth::random_hex(16);
    let url = codex::authorize_url(&pkce.challenge, &oauth_state, "pi");
    let server = callback::start(codex::CALLBACK_PORT, codex::CALLBACK_PATH, &oauth_state).await;
    let notice = format!("Log in to {name} in your browser:\n{url}");
    host.notify_info(notice).await.map_err(|e| e.to_string())?;
    open_browser(host, &url).await;
    let title =
        "Complete login in your browser, or paste the authorization code / redirect URL here:";
    let code = match server {
        Some(mut server) => {
            tokio::select! {
                code = server.wait() => code?,
                input = host.input(title, Some(codex::REDIRECT_URI)) => {
                    pasted_code(input.map_err(|e| e.to_string())?, &oauth_state)?.0
                }
            }
        }
        None => {
            let input = host.input(title, Some(codex::REDIRECT_URI)).await;
            pasted_code(input.map_err(|e| e.to_string())?, &oauth_state)?.0
        }
    };
    codex::exchange(&state.http, &codex::token_url(), &code, &pkce.verifier).await
}

async fn anthropic_login(
    host: &Host,
    state: &State,
    name: &str,
) -> Result<oauth::Credential, String> {
    let pkce = oauth::generate_pkce();
    let url = anthropic::authorize_url(&pkce.challenge, &pkce.verifier);
    let notice =
        format!("Log in to {name} in your browser, then paste the code Anthropic shows:\n{url}");
    host.notify_info(notice).await.map_err(|e| e.to_string())?;
    open_browser(host, &url).await;
    let input = host
        .input(
            "Paste the code Anthropic shows after you sign in:",
            Some("code#state"),
        )
        .await;
    let (code, pasted_state) = pasted_code(input.map_err(|e| e.to_string())?, &pkce.verifier)?;
    let oauth_state = pasted_state.unwrap_or_else(|| pkce.verifier.clone());
    anthropic::exchange(
        &state.http,
        &anthropic::token_url(),
        &code,
        &oauth_state,
        &pkce.verifier,
    )
    .await
}

/// Logs in slot `slot` of `family`, stores the credential, and registers the provider.
pub async fn login(
    host: &Host,
    state: &State,
    family: Family,
    slot: u32,
) -> Result<Account, String> {
    let name = family.slot_name(slot);
    let credential = match family {
        Family::Codex => codex_login(host, state, &name).await?,
        Family::Anthropic => anthropic_login(host, state, &name).await?,
    };
    let account = Account::new(family, slot, credential);
    state
        .accounts
        .upsert(account.clone())
        .await
        .map_err(|e| e.to_string())?;
    state.usage().forget(&account.provider);
    providers::register(host, &account)
        .await
        .map_err(|e| e.to_string())?;
    Ok(account)
}
