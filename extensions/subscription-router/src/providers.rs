//! `provider.register` for extension slots: the built-in provider's models under the slot id,
//! with the slot's access token as `apiKey`, refreshed before expiry.

use std::sync::Arc;
use std::time::Duration;

use pi_ext_sdk::prelude::*;

use crate::accounts::Account;
use crate::family::Family;
use crate::fsutil::now_ms;
use crate::oauth::{Credential, anthropic, codex};
use crate::state::State;

const REFRESH_EVERY: Duration = Duration::from_secs(60);

/// The models of the built-in provider of `family` from pi's registry.
pub async fn base_models(host: &Host, family: Family) -> Result<Vec<Value>> {
    let all = host.pi_call("ctx.modelRegistry", "getAll", vec![]).await?;
    let models = all.as_array().cloned().unwrap_or_default();
    Ok(models
        .into_iter()
        .filter(|m| m.get("provider").and_then(Value::as_str) == Some(family.base()))
        .collect())
}

/// The `provider.register` config of a slot; `None` when pi lists no base models.
pub fn provider_config(
    family: Family,
    slot: u32,
    base_models: &[Value],
    token: &str,
) -> Option<Value> {
    let first = base_models.first()?;
    let provider = family.provider(slot);
    let models: Vec<Value> = base_models
        .iter()
        .map(|model| {
            let mut model = model.clone();
            model["provider"] = json!(provider);
            model
        })
        .collect();
    Some(json!({
        "name": family.slot_name(slot),
        "api": first.get("api").cloned().unwrap_or(Value::Null),
        "baseUrl": first.get("baseUrl").cloned().unwrap_or(Value::Null),
        "apiKey": token,
        "models": models,
    }))
}

pub async fn register(host: &Host, account: &Account) -> Result<()> {
    let models = base_models(host, account.family).await?;
    let config = provider_config(
        account.family,
        account.slot,
        &models,
        &account.credential.access,
    );
    let Some(config) = config else {
        bail!(
            "pi lists no {} models to copy for {}",
            account.family.base(),
            account.provider
        );
    };
    host.register_provider(&account.provider, config).await
}

pub async fn refresh_credential(state: &State, account: &Account) -> Result<Credential, String> {
    let token = &account.credential.refresh;
    let mut credential = match account.family {
        Family::Codex => codex::refresh(&state.http, &codex::token_url(), token).await?,
        Family::Anthropic => {
            anthropic::refresh(&state.http, &anthropic::token_url(), token).await?
        }
    };
    if credential.account_id.is_none() {
        credential.account_id = account.credential.account_id.clone();
    }
    Ok(credential)
}

/// Refreshes `account` when it expires within five minutes, stores it, and re-registers the slot.
/// A failed save is logged; the slot still gets the new token.
pub async fn ensure_fresh(host: &Host, state: &State, account: Account) -> Result<Account> {
    if !account.needs_refresh(now_ms()) {
        return Ok(account);
    }
    let refreshed = state
        .accounts
        .refresh(&account, |current| async move {
            refresh_credential(state, &current).await
        })
        .await
        .map_err(Error::msg)?;
    if let Some(err) = &refreshed.save_error {
        pi_ext_sdk::warn!("cannot save refreshed {}: {err}", account.provider);
    }
    register(host, &refreshed.account).await?;
    Ok(refreshed.account)
}

/// Registers every stored slot, then keeps tokens fresh in the background.
pub async fn start(host: Host, state: Arc<State>) -> Result<()> {
    for account in state.accounts.list() {
        let provider = account.provider.clone();
        let registered = if account.needs_refresh(now_ms()) {
            match ensure_fresh(&host, &state, account.clone()).await {
                Ok(_) => Ok(()),
                Err(err) => {
                    pi_ext_sdk::warn!("cannot refresh {provider}: {err}");
                    register(&host, &account).await
                }
            }
        } else {
            register(&host, &account).await
        };
        if let Err(err) = registered {
            pi_ext_sdk::warn!("cannot register {provider}: {err}");
        }
    }
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(REFRESH_EVERY).await;
            for account in state.accounts.list() {
                if let Err(err) = ensure_fresh(&host, &state, account.clone()).await {
                    pi_ext_sdk::warn!("cannot refresh {}: {err}", account.provider);
                }
            }
        }
    });
    Ok(())
}
