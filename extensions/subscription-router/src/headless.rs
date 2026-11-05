//! Account management without dialogs, for apps that drive pi in RPC mode (such as the Paseo
//! plugin) and for scripts. Each command answers with one JSON notice:
//!
//! - `/router login start <claude|codex>`: `{ id, provider, name, url, paste }`. The user opens
//!   `url`, signs in, and pastes what the provider shows (`code#state`, or the redirect URL).
//! - `/router login finish <id> <pasted>`: `{ provider, name }`. The router keeps the token, as
//!   for its own logins, and refreshes it.
//! - `/router remove <provider> --yes`: `{ removed }`.
//! - `/router resets --json`: `{ accounts: [{ provider, available, credits | error }] }`.
//! - `/router resets <n> --yes`: `{ provider, reset }`.

use std::path::PathBuf;
use std::sync::Arc;

use pi_ext_sdk::prelude::*;
use serde::{Deserialize, Serialize};

use crate::accounts::Account;
use crate::family::{Family, parse_provider};
use crate::fsutil::{now_ms, write_private};
use crate::login::pasted_code;
use crate::oauth::{self, anthropic, codex};
use crate::state::{State, lock};

/// A started login expires after this long.
const LOGIN_TTL_MS: i64 = 30 * 60_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Pending {
    family: Family,
    slot: u32,
    verifier: String,
    state: String,
    created: i64,
}

fn logins_dir(state: &State) -> PathBuf {
    state.agent_dir.join("ext/subscription-router/logins")
}

fn family_of(word: &str) -> Option<Family> {
    match word {
        "claude" | "anthropic" => Some(Family::Anthropic),
        "codex" | "openai-codex" => Some(Family::Codex),
        _ => None,
    }
}

fn pending_logins(state: &State) -> Vec<(String, Pending)> {
    let Ok(entries) = std::fs::read_dir(logins_dir(state)) else {
        return Vec::new();
    };
    let now = now_ms();
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let id = path.file_stem()?.to_str()?.to_owned();
            let pending: Pending = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
            if now - pending.created > LOGIN_TTL_MS {
                std::fs::remove_file(&path).ok();
                return None;
            }
            Some((id, pending))
        })
        .collect()
}

async fn reply(host: &Host, value: Value) -> Result<()> {
    host.notify_info(value.to_string()).await
}

async fn fail(host: &Host, message: impl Into<String>) -> Result<()> {
    host.notify_error(message.into()).await
}

pub async fn login(host: &Host, state: &Arc<State>, args: &str) -> Result<()> {
    let words: Vec<&str> = args.split_whitespace().collect();
    match words.as_slice() {
        ["start", family] => match family_of(family) {
            Some(family) => start(host, state, family).await,
            None => fail(host, "Name claude or codex: /router login start claude").await,
        },
        ["finish", id, pasted @ ..] if !pasted.is_empty() => {
            finish(host, state, id, &pasted.join(" ")).await
        }
        _ => {
            fail(
                host,
                "Use /router login start <claude|codex>, then /router login finish <id> <pasted code or URL>.",
            )
            .await
        }
    }
}

async fn start(host: &Host, state: &Arc<State>, family: Family) -> Result<()> {
    let taken: Vec<u32> = pending_logins(state)
        .into_iter()
        .filter(|(_, p)| p.family == family)
        .map(|(_, p)| p.slot)
        .collect();
    let mut slot = state.accounts.next_slot(family);
    while taken.contains(&slot) || state.accounts.get(&family.provider(slot)).is_some() {
        slot += 1;
    }
    let pkce = oauth::generate_pkce();
    let (oauth_state, url, paste) = match family {
        Family::Anthropic => {
            let url = anthropic::authorize_url(&pkce.challenge, &pkce.verifier);
            (
                pkce.verifier.clone(),
                url,
                "the code Anthropic shows (code#state)",
            )
        }
        Family::Codex => {
            let oauth_state = oauth::random_hex(16);
            let url = codex::authorize_url(&pkce.challenge, &oauth_state, "pi");
            (
                oauth_state,
                url,
                "the address of the page the browser lands on after you sign in (it starts with http://localhost:1455/auth/callback)",
            )
        }
    };
    let id = oauth::random_hex(8);
    let pending = Pending {
        family,
        slot,
        verifier: pkce.verifier,
        state: oauth_state,
        created: now_ms(),
    };
    let path = logins_dir(state).join(format!("{id}.json"));
    if let Err(err) = write_private(&path, serde_json::to_vec(&pending)?.as_slice()) {
        return fail(host, format!("cannot save the login: {err}")).await;
    }
    reply(
        host,
        json!({
            "id": id,
            "provider": family.provider(slot),
            "name": family.slot_name(slot),
            "url": url,
            "paste": paste,
        }),
    )
    .await
}

async fn finish(host: &Host, state: &Arc<State>, id: &str, pasted: &str) -> Result<()> {
    if !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return fail(host, "Unknown login id.").await;
    }
    let path = logins_dir(state).join(format!("{id}.json"));
    let Some((_, pending)) = pending_logins(state).into_iter().find(|(p, _)| p == id) else {
        return fail(
            host,
            "This login expired or does not exist. Start it again.",
        )
        .await;
    };
    let code = match pasted_code(Some(pasted.trim().to_owned()), &pending.state) {
        Ok(code) => code,
        Err(err) => return fail(host, err).await,
    };
    let credential = match pending.family {
        Family::Anthropic => {
            let oauth_state = code.1.unwrap_or_else(|| pending.verifier.clone());
            anthropic::exchange(
                &state.http,
                &anthropic::token_url(),
                &code.0,
                &oauth_state,
                &pending.verifier,
            )
            .await
        }
        Family::Codex => {
            codex::exchange(&state.http, &codex::token_url(), &code.0, &pending.verifier).await
        }
    };
    let credential = match credential {
        Ok(credential) => credential,
        Err(err) => return fail(host, format!("The login failed: {err}")).await,
    };
    std::fs::remove_file(&path).ok();
    let account = Account::new(pending.family, pending.slot, Some(credential));
    if let Err(err) = state.accounts.upsert(account.clone()).await {
        return fail(host, format!("cannot save the account: {err}")).await;
    }
    state.usage().forget(&account.provider);
    if let Err(err) = crate::providers::register(host, state, &account).await {
        pi_ext_sdk::warn!("cannot register {}: {err}", account.provider);
    }
    reply(
        host,
        json!({ "provider": account.provider, "name": account.family.slot_name(account.slot) }),
    )
    .await
}

/// `/router remove <provider> --yes`.
pub async fn remove(host: &Host, state: &Arc<State>, args: &str) -> Result<()> {
    let words: Vec<&str> = args.split_whitespace().collect();
    let Some(provider) = words.iter().find(|w| !w.starts_with("--")) else {
        return fail(host, "Name the account: /router remove anthropic-2 --yes").await;
    };
    if !words.contains(&"--yes") {
        return fail(host, "Add --yes to remove an account without a dialog.").await;
    }
    match parse_provider(provider) {
        Some((_, slot)) if slot > 1 => {}
        _ => {
            return fail(
                host,
                format!("{provider} is not an account the router added."),
            )
            .await;
        }
    }
    host.unregister_provider(provider).await.ok();
    let removed = state.accounts.remove(provider).await?;
    state.usage().forget(provider);
    state.router.tracker().clear_provider(provider);
    if let Some((family, slot)) = parse_provider(provider) {
        let mut picks = lock(&state.picks);
        picks.full_until.remove(&(family, slot));
        if picks.pinned.get(&family) == Some(&slot) {
            picks.pinned.remove(&family);
        }
    }
    reply(host, json!({ "removed": removed, "provider": provider })).await
}

/// `/router resets --json` and `/router resets <n> --yes`.
pub async fn resets(host: &Host, state: &Arc<State>, args: &str) -> Result<()> {
    let words: Vec<&str> = args.split_whitespace().collect();
    let slot = words.iter().find_map(|w| w.parse::<u32>().ok());
    let ids = crate::pick::account_ids(state, Family::Codex);
    if let Some(slot) = slot {
        if !ids.contains(&slot) {
            return fail(host, format!("Codex has accounts {ids:?}.")).await;
        }
        lock(&state.picks)
            .reset_tried
            .remove(&(Family::Codex, slot));
        let reset = crate::resets::try_reset(host, state, Family::Codex, slot).await;
        return reply(
            host,
            json!({ "provider": Family::Codex.provider(slot), "reset": reset }),
        )
        .await;
    }
    let mut accounts = Vec::new();
    for slot in ids {
        let provider = Family::Codex.provider(slot);
        accounts.push(match crate::resets::list(host, state, slot).await {
            Ok(list) => json!({
                "provider": provider,
                "available": list.available_count,
                "credits": list.credits.iter().map(|c| json!({
                    "id": c.id, "type": c.reset_type, "status": c.status,
                    "expiresAt": c.expires_at, "title": c.title,
                })).collect::<Vec<_>>(),
            }),
            Err(error) => json!({ "provider": provider, "available": 0, "error": error }),
        });
    }
    reply(host, json!({ "accounts": accounts })).await
}

/// Whether `/router <action> <args>` is one of these commands.
pub fn handles(action: &str, rest: &str) -> bool {
    match action {
        "login" => true,
        "remove" => true,
        "resets" => rest
            .split_whitespace()
            .any(|w| w == "--json" || w == "--yes"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_by_name() {
        assert_eq!(family_of("claude"), Some(Family::Anthropic));
        assert_eq!(family_of("openai-codex"), Some(Family::Codex));
        assert_eq!(family_of("gemini"), None);
    }
}
