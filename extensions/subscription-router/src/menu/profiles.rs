//! Router profiles: create, edit chain, strategy, exhaustion policy, rename, delete.

use std::sync::Arc;

use pi_ext_sdk::prelude::*;

use super::index_of;
use crate::config::{
    self, ChainEntry, Config, Exhaustion, ModelRef, Policy, Profile, ROUTER_PROVIDER, Strategy,
    THINKING_LEVELS,
};
use crate::routing;
use crate::state::{State, lock};

fn summary(profile: &Profile) -> String {
    let chain: Vec<String> = profile.chain.iter().map(|e| e.model.key()).collect();
    format!(
        "router/{} ({}): {}",
        profile.id,
        profile.strategy().as_str(),
        chain.join(" > ")
    )
}

/// Validates, writes the config file, and registers or unregisters virtual models to match.
pub async fn persist(host: &Host, state: &Arc<State>, next: Config) -> Result<()> {
    let path = config::save(&state.agent_dir, &next).map_err(Error::msg)?;
    let previous = std::mem::replace(&mut *lock(&state.config), next.clone());
    *lock(&state.config_error) = None;
    for old in &previous.profiles {
        let renamed = next.profile(&old.id).is_some_and(|p| p.name != old.name);
        if next.profile(&old.id).is_none() || renamed {
            host.unregister_virtual_model(ROUTER_PROVIDER, &old.id)
                .await?;
            lock(&state.registered).remove(&old.id);
        }
    }
    for profile in &next.profiles {
        if !lock(&state.registered).insert(profile.id.clone()) {
            continue;
        }
        let route_state = state.clone();
        let route = move |request, host| routing::route(route_state.clone(), request, host);
        host.register_virtual_model(routing::definition(profile), route)
            .await?;
    }
    host.notify_info(format!("subscription-router: saved {}", path.display()))
        .await
}

pub async fn list(host: &Host, state: &Arc<State>) -> Result<()> {
    loop {
        let config = state.config();
        let mut options: Vec<String> = config.profiles.iter().map(summary).collect();
        options.push("New profile".into());
        match index_of(&options, host.select("Router profiles", &options).await?) {
            None => return Ok(()),
            Some(i) if i == config.profiles.len() => create(host, state).await?,
            Some(i) => edit(host, state, config.profiles[i].id.clone()).await?,
        }
    }
}

async fn create(host: &Host, state: &Arc<State>) -> Result<()> {
    let Some(id) = host
        .input("Profile id (lowercase, for router/<id>)", Some("default"))
        .await?
    else {
        return Ok(());
    };
    let id = id.trim().to_owned();
    if state.config().profile(&id).is_some() {
        return host
            .notify_error(format!("router/{id} already exists."))
            .await;
    }
    let mut profile = Profile {
        id: id.clone(),
        name: format!("Router {id}"),
        strategy: None,
        chain: Vec::new(),
        exhaustion: None,
    };
    edit_chain(host, &mut profile).await?;
    if profile.chain.is_empty() {
        return host
            .notify_warning("A profile needs at least one chain model; nothing saved.")
            .await;
    }
    let mut next = state.config();
    next.profiles.push(profile);
    if let Err(err) = config::parse_config(&config::config_json(&next)) {
        return host
            .notify_error(format!("subscription-router: {err}"))
            .await;
    }
    persist(host, state, next).await
}

async fn edit(host: &Host, state: &Arc<State>, id: String) -> Result<()> {
    loop {
        let Some(mut profile) = state.config().profile(&id).cloned() else {
            return Ok(());
        };
        let policy = match &profile.exhaustion {
            Some(e) if e.policy == Policy::Fail => "fail",
            _ => "wait",
        };
        let wait = profile.exhaustion.as_ref().and_then(|e| e.max_wait_minutes);
        let options = [
            "Edit chain".to_owned(),
            format!("Strategy: {}", profile.strategy().as_str()),
            format!("Exhaustion policy: {policy}"),
            format!(
                "Max wait minutes: {}",
                wait.map_or("no limit".into(), |m| m.to_string())
            ),
            format!("Rename ({})", profile.name),
            "Delete".to_owned(),
        ];
        let mut next = state.config();
        match index_of(
            &options,
            host.select(&format!("router/{id}"), &options).await?,
        ) {
            None => return Ok(()),
            Some(0) => edit_chain(host, &mut profile).await?,
            Some(1) => {
                let next_strategy = if profile.strategy() == Strategy::Ordered {
                    Strategy::Balanced
                } else {
                    Strategy::Ordered
                };
                profile.strategy = Some(next_strategy);
            }
            Some(2) => {
                let policy = if policy == "wait" {
                    Policy::Fail
                } else {
                    Policy::Wait
                };
                profile.exhaustion = Some(Exhaustion {
                    policy,
                    max_wait_minutes: wait,
                });
            }
            Some(3) => {
                let Some(text) = host
                    .input("Max wait minutes (empty for no limit)", Some("360"))
                    .await?
                else {
                    continue;
                };
                let minutes = text.trim().parse::<f64>().ok().filter(|m| *m > 0.0);
                let policy = profile
                    .exhaustion
                    .as_ref()
                    .map_or(Policy::Wait, |e| e.policy);
                profile.exhaustion = Some(Exhaustion {
                    policy,
                    max_wait_minutes: minutes,
                });
            }
            Some(4) => {
                let Some(name) = host.input("Profile name", Some(&profile.name)).await? else {
                    continue;
                };
                profile.name = name.trim().to_owned();
            }
            Some(_) => {
                if host
                    .confirm("Delete profile", &format!("Delete router/{id}?"))
                    .await?
                {
                    next.profiles.retain(|p| p.id != id);
                    return persist(host, state, next).await;
                }
                continue;
            }
        }
        if profile.chain.is_empty() {
            host.notify_warning("A profile needs at least one chain model; nothing saved.")
                .await?;
            continue;
        }
        next.profiles
            .iter_mut()
            .filter(|p| p.id == id)
            .for_each(|p| *p = profile.clone());
        persist(host, state, next).await?;
    }
}

async fn pick_model(host: &Host) -> Result<Option<ModelRef>> {
    let all = host
        .pi_call("ctx.modelRegistry", "getAll", vec![])
        .await
        .unwrap_or(Value::Null);
    let mut options: Vec<String> = all
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            Some(format!(
                "{}/{}",
                m.get("provider")?.as_str()?,
                m.get("id")?.as_str()?
            ))
        })
        .filter(|key| !key.starts_with("router/"))
        .collect();
    options.push("Type provider/model-id".into());
    let Some(index) = index_of(&options, host.select("Add chain model", &options).await?) else {
        return Ok(None);
    };
    if index + 1 < options.len() {
        return Ok(ModelRef::parse(&options[index]));
    }
    let typed = host
        .input(
            "Chain model (provider/model-id)",
            Some("openai-codex/gpt-5.5"),
        )
        .await?;
    Ok(typed
        .as_deref()
        .map(str::trim)
        .and_then(ModelRef::parse)
        .filter(|m| m.provider != ROUTER_PROVIDER))
}

async fn edit_chain(host: &Host, profile: &mut Profile) -> Result<()> {
    loop {
        let mut options: Vec<String> = profile
            .chain
            .iter()
            .enumerate()
            .map(|(i, e)| {
                format!(
                    "{}. {}{}",
                    i + 1,
                    e.model.key(),
                    e.thinking
                        .as_ref()
                        .map_or(String::new(), |t| format!(" ({t})"))
                )
            })
            .collect();
        options.extend(["Add model".to_owned(), "Done".to_owned()]);
        let len = profile.chain.len();
        match index_of(
            &options,
            host.select(&format!("router/{} chain", profile.id), &options)
                .await?,
        ) {
            None => return Ok(()),
            Some(i) if i == len + 1 => return Ok(()),
            Some(i) if i == len => {
                if let Some(model) = pick_model(host).await? {
                    if profile.entry(&model).is_none() {
                        profile.chain.push(ChainEntry {
                            model,
                            thinking: None,
                        });
                    }
                }
            }
            Some(i) => {
                let actions = ["Move up", "Move down", "Set thinking level", "Remove"];
                match index_of(
                    &actions,
                    host.select(&profile.chain[i].model.key(), &actions).await?,
                ) {
                    Some(0) if i > 0 => profile.chain.swap(i, i - 1),
                    Some(1) if i + 1 < len => profile.chain.swap(i, i + 1),
                    Some(2) => {
                        let mut levels: Vec<&str> = vec!["inherit"];
                        levels.extend(THINKING_LEVELS);
                        let level =
                            index_of(&levels, host.select("Thinking level", &levels).await?);
                        if let Some(level) = level {
                            profile.chain[i].thinking =
                                (level > 0).then(|| levels[level].to_owned());
                        }
                    }
                    Some(3) => drop(profile.chain.remove(i)),
                    _ => {}
                }
            }
        }
    }
}
