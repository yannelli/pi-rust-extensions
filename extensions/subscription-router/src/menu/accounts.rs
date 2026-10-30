//! Account slots: list with quota, add and log in, re-login, remove, switch.

use std::sync::Arc;

use pi_ext_sdk::prelude::*;

use super::index_of;
use crate::family::Family;
use crate::fsutil::now_ms;
use crate::slots::{SlotInfo, status_line};
use crate::state::{HostTokens, State, current_model};
use crate::usage::{GetOptions, format};

pub async fn list(host: &Host, state: &Arc<State>) -> Result<()> {
    loop {
        let slots: Vec<SlotInfo> = Family::ALL
            .into_iter()
            .flat_map(|f| state.slots(f))
            .collect();
        let mut options: Vec<String> = slots.iter().map(|s| status_line(s, now_ms())).collect();
        options.push("Add account".into());
        let choice = host.select("Accounts", &options).await?;
        match index_of(&options, choice) {
            None => return Ok(()),
            Some(i) if i == slots.len() => add(host, state).await?,
            Some(i) => slot_menu(host, state, slots[i].family, slots[i].slot).await?,
        }
    }
}

pub async fn add(host: &Host, state: &Arc<State>) -> Result<()> {
    let families: Vec<&str> = Family::ALL.iter().map(|f| f.label()).collect();
    let Some(index) = index_of(&families, host.select("Add account", &families).await?) else {
        return Ok(());
    };
    let family = Family::ALL[index];
    let slot = state.accounts.next_slot(family);
    match crate::login::login(host, state, family, slot).await {
        Ok(account) => {
            let message = format!(
                "Added {} as provider {}.",
                family.slot_name(slot),
                account.provider
            );
            host.notify_info(message).await
        }
        Err(err) => {
            host.notify_error(format!("{} login failed: {err}", family.slot_name(slot)))
                .await
        }
    }
}

async fn relogin(host: &Host, state: &Arc<State>, family: Family, slot: u32) -> Result<()> {
    if slot == 1 {
        let message = format!(
            "Slot 1 is pi's {} provider. Run /login and pick it.",
            family.base()
        );
        return host.notify_info(message).await;
    }
    match crate::login::login(host, state, family, slot).await {
        Ok(_) => {
            host.notify_info(format!("Logged in to {}.", family.slot_name(slot)))
                .await
        }
        Err(err) => {
            host.notify_error(format!("{} login failed: {err}", family.slot_name(slot)))
                .await
        }
    }
}

async fn remove(host: &Host, state: &Arc<State>, family: Family, slot: u32) -> Result<bool> {
    let name = family.slot_name(slot);
    if slot == 1 {
        let message = format!(
            "Slot 1 is pi's {} provider. Run /logout to remove it.",
            family.base()
        );
        host.notify_info(message).await?;
        return Ok(false);
    }
    let message = format!("Unregister {name} and delete its stored credentials?");
    if !host.confirm("Remove account", &message).await? {
        return Ok(false);
    }
    let provider = family.provider(slot);
    host.unregister_provider(&provider).await?;
    state.accounts.remove(&provider).await?;
    state.usage().forget(&provider);
    state.router.tracker().clear_provider(&provider);
    let current = current_model(host).is_some_and(|m| m.provider == provider);
    let suffix = if current {
        " Select another model with /model."
    } else {
        ""
    };
    host.notify_info(format!("Removed {name}.{suffix}")).await?;
    Ok(true)
}

async fn view(
    host: &Host,
    state: &Arc<State>,
    family: Family,
    slot: u32,
    refresh: bool,
) -> Result<()> {
    let provider = family.provider(slot);
    let snapshot = if refresh {
        let tokens = HostTokens { host, state };
        let options = GetOptions {
            force: true,
            ..Default::default()
        };
        Some(state.usage().get(&provider, &tokens, options).await)
    } else {
        state.usage().peek(&provider)
    };
    let info = SlotInfo {
        family,
        slot,
        snapshot: snapshot.clone(),
    };
    let lines = [
        status_line(&info, now_ms()),
        format!("{provider}: {}", format(snapshot.as_ref())),
    ];
    host.notify_info(lines.join("\n")).await
}

async fn switch(host: &Host, state: &Arc<State>, family: Family, slot: u32) -> Result<()> {
    let provider = family.provider(slot);
    let Some(current) = current_model(host)
        .filter(|m| crate::family::parse_provider(&m.provider).is_some_and(|(f, _)| f == family))
    else {
        return host
            .notify_warning(format!(
                "Select an {} model first, then switch slots.",
                family.label()
            ))
            .await;
    };
    if !crate::failover::switch_to(host, state, &provider, &current.id).await {
        return host
            .notify_warning(format!(
                "pi has no {provider}/{} with credentials.",
                current.id
            ))
            .await;
    }
    crate::status::update(host, state).await;
    host.notify_info(format!("Switched to {}.", family.slot_name(slot)))
        .await
}

async fn slot_menu(host: &Host, state: &Arc<State>, family: Family, slot: u32) -> Result<()> {
    let options = [
        "View quota",
        "Refresh quota",
        "Switch to this slot",
        "Log in again",
        "Remove",
    ];
    loop {
        let title = family.slot_name(slot);
        match index_of(&options, host.select(&title, &options).await?) {
            None => return Ok(()),
            Some(0) => view(host, state, family, slot, false).await?,
            Some(1) => view(host, state, family, slot, true).await?,
            Some(2) => switch(host, state, family, slot).await?,
            Some(3) => relogin(host, state, family, slot).await?,
            Some(_) => {
                if remove(host, state, family, slot).await? {
                    return Ok(());
                }
            }
        }
    }
}
