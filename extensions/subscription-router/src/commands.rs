//! `/router` actions, `/codex-status`, and `/codex-switch`.

use std::sync::Arc;

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::{CompletionItem, NotifyType};

use crate::config::ModelRef;
use crate::family::{Family, parse_provider};
use crate::fsutil::now_ms;
use crate::router::{Pick, format_decision};
use crate::selection::format_score;
use crate::slots::status_line;
use crate::state::{HostEnv, HostTokens, State, current_model, lock};
use crate::usage::{GetOptions, format};

pub const ACTIONS: [(&str, &str); 7] = [
    (
        "status",
        "cached usage, each profile's pick, and recent decisions",
    ),
    ("refresh", "read usage now, then show status"),
    ("reset", "clear confirmed exhaustion blocks"),
    ("manage", "accounts and profiles menu"),
    ("accounts", "list, add, log in, remove, or switch slots"),
    ("profiles", "create, edit, or delete router profiles"),
    ("add", "add a Codex or Anthropic account and log in"),
];

pub fn completions(prefix: &str) -> Option<Vec<CompletionItem>> {
    let items: Vec<CompletionItem> = ACTIONS
        .iter()
        .filter(|(name, _)| name.starts_with(prefix.trim()))
        .map(|(name, description)| CompletionItem::new(*name).description(*description))
        .collect();
    (!items.is_empty()).then_some(items)
}

/// The `/router status` text: usage per provider, profile forecasts, and the last decisions.
pub async fn status_report(host: &Host, state: &Arc<State>, refresh: bool) -> String {
    let providers = state.session_providers(current_model(host).map(|m| m.provider).as_deref());
    let mut lines = Vec::new();
    let tokens = HostTokens { host, state };
    for provider in &providers {
        let snapshot = if refresh {
            let options = GetOptions {
                force: true,
                ..Default::default()
            };
            Some(state.usage().get(provider, &tokens, options).await)
        } else {
            state.usage().peek(provider)
        };
        if let Some(snapshot) = snapshot {
            lines.push(format!("{provider}: {}", format(Some(&snapshot))));
        }
    }
    let config = state.config();
    let models: Vec<ModelRef> = config
        .profiles
        .iter()
        .flat_map(|p| p.chain.iter().map(|e| e.model.clone()))
        .collect();
    let env = HostEnv::build(host, state, &models).await;
    for profile in &config.profiles {
        let strategy = profile.strategy().as_str();
        match state.router.pick(profile, &env, None) {
            Pick::Chosen {
                entry,
                candidates,
                blocked,
            } => {
                lines.push(format!(
                    "router/{} ({strategy}) -> {}",
                    profile.id,
                    entry.model.key()
                ));
                for c in candidates {
                    lines.push(format!(
                        "  {}: {}",
                        c.entry.model.key(),
                        format_score(&c.score)
                    ));
                }
                blocked
                    .iter()
                    .for_each(|b| lines.push(format!("  {}: {}", b.entry.model.key(), b.reason)));
            }
            Pick::Exhausted { blocked, .. } => {
                lines.push(format!("router/{} ({strategy}) -> exhausted", profile.id));
                blocked
                    .iter()
                    .for_each(|b| lines.push(format!("  {}: {}", b.entry.model.key(), b.reason)));
            }
        }
    }
    let recent: Vec<String> = lock(&state.recent).iter().map(format_decision).collect();
    if !recent.is_empty() {
        lines.push("last decisions (newest last):".into());
        lines.extend(recent);
    }
    if let Some(error) = lock(&state.config_error).as_ref() {
        lines.push(format!("config ignored: {error}"));
    }
    if lines.is_empty() {
        "subscription-router: no subscription providers or profiles.".into()
    } else {
        lines.join("\n")
    }
}

pub async fn router(state: Arc<State>, args: String, ctx: CommandCtx) -> Result<()> {
    let host = ctx.host();
    match args.trim() {
        "" | "status" => {
            host.notify_info(status_report(host, &state, false).await)
                .await
        }
        "refresh" => {
            host.notify_info(status_report(host, &state, true).await)
                .await
        }
        "reset" => {
            state.router.tracker().clear();
            host.notify_info("subscription-router: cleared confirmed blocks")
                .await
        }
        "manage" | "menu" => crate::menu::open(host, &state).await,
        "accounts" => crate::menu::accounts::list(host, &state).await,
        "add" => crate::menu::accounts::add(host, &state).await,
        "profiles" => crate::menu::profiles::list(host, &state).await,
        other => {
            let names: Vec<&str> = ACTIONS.iter().map(|(n, _)| *n).collect();
            let message = format!(
                "subscription-router: unknown action \"{other}\" ({})",
                names.join(", ")
            );
            host.notify_as(message, NotifyType::Error).await
        }
    }
}

pub fn family_status(state: &State, family: Family) -> Vec<String> {
    let now = now_ms();
    state
        .slots(family)
        .iter()
        .map(|info| status_line(info, now))
        .collect()
}

pub async fn codex_status(state: Arc<State>, _args: String, ctx: CommandCtx) -> Result<()> {
    ctx.host()
        .notify_info(family_status(&state, Family::Codex).join("\n"))
        .await
}

/// Moves the current model of `family` to another slot of that family.
pub async fn switch_slot(host: &Host, state: &Arc<State>, family: Option<Family>) -> Result<()> {
    let current = current_model(host);
    let parsed = current.as_ref().and_then(|m| parse_provider(&m.provider));
    let Some((current, (current_family, _))) = current
        .zip(parsed)
        .filter(|(_, (f, _))| family.is_none_or(|x| x == *f))
    else {
        let label = family.map_or("an OpenAI Codex or Anthropic", Family::label);
        return host
            .notify_warning(format!("Select {label} model before switching slots."))
            .await;
    };
    let slots = state.slots(current_family);
    let choices: Vec<String> = slots.iter().map(|s| status_line(s, now_ms())).collect();
    let title = format!("Switch {} slot", current_family.label());
    let Some(choice) = host.select(&title, &choices).await? else {
        return Ok(());
    };
    let Some(index) = choices.iter().position(|c| *c == choice) else {
        return Ok(());
    };
    let provider = slots[index].provider();
    if provider != current.provider
        && !crate::failover::switch_to(host, state, &provider, &current.id).await
    {
        return host
            .notify_warning(format!(
                "pi has no {}/{} with credentials.",
                provider, current.id
            ))
            .await;
    }
    crate::status::update(host, state).await;
    Ok(())
}

pub async fn codex_switch(state: Arc<State>, _args: String, ctx: CommandCtx) -> Result<()> {
    switch_slot(ctx.host(), &state, Some(Family::Codex)).await
}
