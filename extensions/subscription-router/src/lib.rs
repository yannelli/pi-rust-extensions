//! subscription-router: several ChatGPT Codex and Anthropic subscription accounts, picked per
//! request by a policy (manual, fill, threshold, balance) with failover, usage resets, and
//! fallback to other providers, and `router/<profile>` virtual models. On pi without the
//! `providerAuth` feature the accounts are provider slots (`anthropic-2`) with copied models.

pub mod accounts;
pub mod commands;
pub mod config;
pub mod exhaustion;
pub mod failover;
pub mod family;
pub mod filelock;
pub mod fsutil;
pub mod headless;
pub mod login;
pub mod menu;
mod model_cache;
pub mod oauth;
pub mod pick;
pub mod providers;
pub mod resets;
pub mod router;
pub mod routing;
pub mod selection;
pub mod slots;
pub mod state;
pub mod status;
pub mod tool;
pub mod usage;

use std::process::ExitCode;
use std::sync::Arc;

use pi_ext_sdk::prelude::*;

use crate::state::{State, current_model, lock, warm};

/// The `initialize` feature of pi that sends `before_provider_auth`.
pub const PROVIDER_AUTH: &str = "providerAuth";

pub fn main() -> ExitCode {
    let state = Arc::new(State::new(&pi_ext_sdk::paths::agent_dir()));
    extension(state).run()
}

async fn session_start(host: Host, state: Arc<State>) -> Result<()> {
    lock(&state.persisted).clear();
    let error = lock(&state.config_error).clone();
    if let Some(error) = error {
        host.notify_error(format!("subscription-router: ignoring config. {error}"))
            .await
            .ok();
    }
    crate::status::update(&host, &state).await;
    warm(
        &host,
        &state,
        state.session_providers(current_model(&host).map(|m| m.provider).as_deref()),
    );
    Ok(())
}

async fn start(host: Host, state: Arc<State>) -> Result<()> {
    let accounts = host
        .init()
        .is_some_and(|init| init.has_feature(PROVIDER_AUTH));
    state
        .accounts_mode
        .store(accounts, std::sync::atomic::Ordering::Relaxed);
    for channel in [tool::RPC_ADMIT, tool::RPC_USAGE] {
        let (host, state) = (host.clone(), state.clone());
        tokio::spawn(async move {
            if let Err(err) = tool::serve_rpc(host, state, channel).await {
                pi_ext_sdk::warn!("cannot serve {channel}: {err}");
            }
        });
    }
    providers::start(host.clone(), state.clone()).await?;
    crate::status::update(&host, &state).await;
    Ok(())
}

/// The extension with every handler; `main` serves it on stdio, tests on a duplex.
pub fn extension(state: Arc<State>) -> Extension {
    let mut ext = Extension::new("subscription-router");
    let cache = model_cache::ModelCache::load(&state.agent_dir);
    let declared = state.clone();
    ext = ext.providers_with(move |init| {
        let accounts = init.has_feature(PROVIDER_AUTH);
        declared
            .accounts_mode
            .store(accounts, std::sync::atomic::Ordering::Relaxed);
        declared
            .accounts
            .list()
            .into_iter()
            .filter_map(|account| {
                let token = account.credential.as_ref().map(|c| c.access.as_str());
                let config = if accounts {
                    Some(providers::account_config(
                        account.family,
                        account.slot,
                        token,
                    ))
                } else {
                    providers::provider_config(
                        account.family,
                        account.slot,
                        cache.models(account.family),
                        token,
                    )
                };
                Some((account.provider.clone(), config?))
            })
            .collect()
    });
    for profile in state.config().profiles {
        let s = state.clone();
        ext = ext.virtual_model(routing::definition(&profile), move |request, host| {
            routing::route(s.clone(), request, host)
        });
    }
    let (s1, s2, s3, s4, s5, s6) = (
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
    );
    let s13 = state.clone();
    let s14 = state.clone();
    let (s7, s8, s9, s10, s11, s12) = (
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
    );
    ext.tool(tool::spec(), move |args: tool::UsageArgs, ctx: ToolCtx| {
        tool::run(s1.clone(), args, ctx.host().clone())
    })
    .command(
        "router",
        "Subscription router: status (default), refresh, reset, manage, accounts, profiles, add",
        move |args, ctx| commands::router(s2.clone(), args, ctx),
    )
    .completions("router", |prefix, _host| async move {
        Ok(commands::completions(&prefix))
    })
    .command(
        "codex-status",
        "Show Codex quota per account slot",
        move |args, ctx| commands::codex_status(s3.clone(), args, ctx),
    )
    .command(
        "codex-switch",
        "Switch the current Codex model to another account slot",
        move |args, ctx| commands::codex_switch(s4.clone(), args, ctx),
    )
    .on("session_start", move |ev: EventCtx| {
        session_start(ev.host().clone(), s5.clone())
    })
    .on_start(move |host| start(host, s6.clone()))
    .on("before_agent_start", move |ev: EventCtx| {
        tool::before_agent_start(ev.host().clone(), s7.clone())
    })
    .on("before_provider_request", move |ev: EventCtx| {
        let state = s13.clone();
        async move {
            lock(&state.failover).request_provider = current_model(ev.host()).map(|m| m.provider);
            Ok(())
        }
    })
    .on("after_provider_response", move |ev: EventCtx| {
        let state = s8.clone();
        async move {
            lock(&state.failover).status = ev.get("status").and_then(Value::as_u64);
            Ok(())
        }
    })
    .on("before_provider_auth", move |ev: EventCtx| {
        pick::on_auth(ev.host().clone(), s14.clone(), ev.event().clone())
    })
    .on("message_end", move |ev: EventCtx| {
        let (host, state, event) = (ev.host().clone(), s9.clone(), ev.event().clone());
        async move {
            if pick::enabled(&state) {
                pick::on_message_end(host, state, event).await
            } else {
                failover::on_message_end(host, state, event).await
            }
        }
    })
    .on("agent_before_settle", move |ev: EventCtx| {
        routing::on_before_settle(ev.host().clone(), s10.clone(), ev.event().clone())
    })
    .on("agent_settled", move |ev: EventCtx| {
        let state = s11.clone();
        async move {
            lock(&state.failover).tried.clear();
            crate::status::update(ev.host(), &state).await;
            Ok(())
        }
    })
    .on("model_select", move |ev: EventCtx| {
        let state = s12.clone();
        async move {
            let host = ev.host().clone();
            let by_user = {
                let mut failover = lock(&state.failover);
                if !failover.selecting {
                    failover.explicit_choice = true;
                }
                !failover.selecting
            };
            if by_user {
                pick::user_selected(&state);
            }
            crate::status::update(&host, &state).await;
            warm(
                &host,
                &state,
                state.session_providers(current_model(&host).map(|m| m.provider).as_deref()),
            );
            Ok(())
        }
    })
}
