//! `/router use` (the account of this session), `/router policy` (how each provider picks its
//! accounts), and `/router resets` (banked Codex usage resets).

use std::sync::Arc;

use pi_ext_sdk::prelude::*;

use super::index_of;
use crate::config::{
    AccountPolicy, AccountStrategy, ChainEntry, ModelRef, ROUTER_PROVIDER, ResetUse,
};
use crate::family::{Family, parse_provider};
use crate::fsutil::{duration, now_ms};
use crate::menu::profiles::persist;
use crate::pick::{account_ids, account_name, enabled};
use crate::slots::{SlotInfo, status_text};
use crate::state::{State, lock};

const STRATEGY_HELP: [(AccountStrategy, &str); 4] = [
    (
        AccountStrategy::Balance,
        "balance: the account with the most quota left for the time left",
    ),
    (
        AccountStrategy::Fill,
        "fill: stay on an account until it is full, then the next",
    ),
    (
        AccountStrategy::Threshold,
        "threshold: like fill, switching at a percent you set",
    ),
    (
        AccountStrategy::Manual,
        "manual: always the account you choose",
    ),
];

const RESET_HELP: [(ResetUse, &str); 3] = [
    (ResetUse::Off, "off: never spend a usage reset"),
    (
        ResetUse::WhenFull,
        "when full: as soon as the account in use is full",
    ),
    (
        ResetUse::WhenAllFull,
        "when all full: only when every account is full",
    ),
];

fn family_of(text: &str) -> Option<Family> {
    match text {
        "claude" | "anthropic" => Some(Family::Anthropic),
        "codex" | "openai-codex" | "openai" => Some(Family::Codex),
        _ => None,
    }
}

async fn need_accounts(host: &Host, state: &State) -> Result<bool> {
    if enabled(state) {
        return Ok(true);
    }
    host.notify_warning("This pi has no providerAuth: accounts are separate providers here, so switch them with /router accounts.")
        .await?;
    Ok(false)
}

async fn pick_family(host: &Host, title: &str) -> Result<Option<Family>> {
    let options = [Family::Anthropic.label(), Family::Codex.label()];
    Ok(
        match index_of(&options, host.select(title, &options).await?) {
            Some(0) => Some(Family::Anthropic),
            Some(1) => Some(Family::Codex),
            _ => None,
        },
    )
}

fn account_line(state: &State, family: Family, slot: u32) -> String {
    let info = SlotInfo {
        family,
        slot,
        snapshot: state.usage().peek(&family.provider(slot)),
    };
    let login = if slot == 1 { " (pi login)" } else { "" };
    format!("{}{login}", status_text(&info, now_ms()))
}

/// `/router use [claude|codex] [<n>|auto]`: pins an account for this session, or clears the pin.
pub async fn use_account(host: &Host, state: &Arc<State>, args: &str) -> Result<()> {
    if !need_accounts(host, state).await? {
        return Ok(());
    }
    let mut words = args.split_whitespace();
    let family = match words.next() {
        Some(word) => family_of(word),
        None => pick_family(host, "Use an account of").await?,
    };
    let Some(family) = family else {
        return host
            .notify_warning("Name claude or codex, for example /router use claude 2.")
            .await;
    };
    let ids = account_ids(state, family);
    let policy = state.config().policy(family);
    let choice = match words.next() {
        Some("auto") => Some(None),
        Some(word) => match word.parse::<u32>().ok().filter(|n| ids.contains(n)) {
            Some(slot) => Some(Some(slot)),
            None => {
                let message = format!("{} has accounts {ids:?}.", family.label());
                return host.notify_warning(message).await;
            }
        },
        None => {
            let mut options = vec![format!("Automatic ({})", policy.strategy.as_str())];
            options.extend(ids.iter().map(|slot| account_line(state, family, *slot)));
            let title = format!("{} account for this session", family.label());
            match index_of(&options, host.select(&title, &options).await?) {
                None => None,
                Some(0) => Some(None),
                Some(index) => Some(Some(ids[index - 1])),
            }
        }
    };
    let Some(choice) = choice else {
        return Ok(());
    };
    let message = {
        let mut picks = lock(&state.picks);
        match choice {
            Some(slot) => {
                picks.pinned.insert(family, slot);
                picks.current.insert(family, slot);
                format!(
                    "This session uses {} for {} models.",
                    account_name(family, slot),
                    family.base()
                )
            }
            None => {
                picks.pinned.remove(&family);
                format!(
                    "{} accounts follow the {} policy again.",
                    family.label(),
                    policy.strategy.as_str()
                )
            }
        }
    };
    crate::status::update(host, state).await;
    host.notify_info(message).await
}

/// Models pi lists outside `family`, for fallbacks.
async fn other_models(host: &Host, family: Family) -> Vec<String> {
    let all = host
        .pi_call("ctx.modelRegistry", "getAvailable", vec![])
        .await;
    let all = match all {
        Ok(all) => all,
        Err(_) => host
            .pi_call("ctx.modelRegistry", "getAll", vec![])
            .await
            .unwrap_or_default(),
    };
    let mut models: Vec<String> = all
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let provider = m.get("provider")?.as_str()?;
            let id = m.get("id")?.as_str()?;
            let own = parse_provider(provider).is_some_and(|(f, _)| f == family);
            (!own && provider != ROUTER_PROVIDER).then(|| format!("{provider}/{id}"))
        })
        .collect();
    models.sort();
    models.dedup();
    models
}

fn summary(policy: &AccountPolicy, family: Family) -> Vec<String> {
    let mut lines = vec![format!("Strategy: {}", policy.strategy.as_str())];
    if policy.strategy == AccountStrategy::Manual {
        lines.push(format!("Account: {}", account_name(family, policy.account)));
    }
    if policy.strategy == AccountStrategy::Threshold {
        lines.push(format!("Switch at: {}%", policy.switch_at));
    }
    let fallback: Vec<String> = policy.fallback.iter().map(|e| e.model.key()).collect();
    lines.push(format!(
        "When all accounts are full: {}",
        if fallback.is_empty() {
            "stop".into()
        } else {
            fallback.join(" > ")
        }
    ));
    lines.push(format!(
        "Return when an account has room: {}",
        if policy.return_when_available {
            "on"
        } else {
            "off"
        }
    ));
    if family == Family::Codex {
        lines.push(format!("Usage resets: {}", policy.use_resets.as_str()));
        lines.push(format!(
            "Skip a reset when the window frees up within: {}",
            duration((policy.reset_min_wait_minutes * 60_000.0) as i64)
        ));
    }
    lines
}

async fn save(
    host: &Host,
    state: &Arc<State>,
    family: Family,
    policy: AccountPolicy,
) -> Result<()> {
    let mut next = state.config();
    next.accounts.insert(family.base(), policy);
    persist(host, state, next).await
}

/// `/router policy [claude|codex]`: the account policy of one provider, saved at each change.
pub async fn policy(host: &Host, state: &Arc<State>, args: &str) -> Result<()> {
    let family = match args.split_whitespace().next() {
        Some(word) => family_of(word),
        None => pick_family(host, "Account policy of").await?,
    };
    let Some(family) = family else {
        return Ok(());
    };
    loop {
        let policy = state.config().policy(family);
        let lines = summary(&policy, family);
        let title = format!("{} accounts", family.label());
        let Some(index) = index_of(&lines, host.select(&title, &lines).await?) else {
            return Ok(());
        };
        let label = lines[index]
            .split(':')
            .next()
            .unwrap_or_default()
            .to_owned();
        let mut next = policy.clone();
        match label.as_str() {
            "Strategy" => {
                let options: Vec<&str> = STRATEGY_HELP.iter().map(|(_, help)| *help).collect();
                let Some(i) = index_of(&options, host.select("Strategy", &options).await?) else {
                    continue;
                };
                next.strategy = STRATEGY_HELP[i].0;
                if next.strategy == AccountStrategy::Manual {
                    next.account = choose_account(host, state, family)
                        .await?
                        .unwrap_or(next.account);
                }
            }
            "Account" => {
                next.account = choose_account(host, state, family)
                    .await?
                    .unwrap_or(next.account);
            }
            "Switch at" => {
                let answer = host
                    .input("Switch accounts at this percent used", Some("90"))
                    .await?;
                match answer
                    .as_deref()
                    .map(str::trim)
                    .and_then(|a| a.trim_end_matches('%').parse::<f64>().ok())
                {
                    Some(percent) if percent > 0.0 && percent <= 100.0 => next.switch_at = percent,
                    Some(_) => {
                        host.notify_warning("Enter a percent above 0, at most 100.")
                            .await?
                    }
                    None => continue,
                }
            }
            "When all accounts are full" => {
                if !fallback_menu(host, &mut next, family).await? {
                    continue;
                }
            }
            "Return when an account has room" => {
                next.return_when_available = !next.return_when_available
            }
            "Usage resets" => {
                let options: Vec<&str> = RESET_HELP.iter().map(|(_, help)| *help).collect();
                let Some(i) = index_of(&options, host.select("Usage resets", &options).await?)
                else {
                    continue;
                };
                next.use_resets = RESET_HELP[i].0;
            }
            "Skip a reset when the window frees up within" => {
                let answer = host.input("Minutes", Some("60")).await?;
                match answer
                    .as_deref()
                    .map(str::trim)
                    .and_then(|a| a.parse::<f64>().ok())
                {
                    Some(minutes) if minutes >= 0.0 => next.reset_min_wait_minutes = minutes,
                    Some(_) => host.notify_warning("Enter a number of minutes.").await?,
                    None => continue,
                }
            }
            _ => continue,
        }
        if next != policy {
            save(host, state, family, next).await?;
        }
    }
}

async fn choose_account(host: &Host, state: &State, family: Family) -> Result<Option<u32>> {
    let ids = account_ids(state, family);
    let options: Vec<String> = ids
        .iter()
        .map(|slot| account_line(state, family, *slot))
        .collect();
    Ok(index_of(&options, host.select("Account", &options).await?).map(|i| ids[i]))
}

/// Adds or clears fallback models; returns whether `policy` changed.
async fn fallback_menu(host: &Host, policy: &mut AccountPolicy, family: Family) -> Result<bool> {
    let mut options = vec!["Add a fallback model".to_owned()];
    options.extend(
        policy
            .fallback
            .iter()
            .map(|e| format!("Remove {}", e.model.key())),
    );
    let Some(index) = index_of(
        &options,
        host.select("Fallback models, in order", &options).await?,
    ) else {
        return Ok(false);
    };
    if index > 0 {
        policy.fallback.remove(index - 1);
        return Ok(true);
    }
    let models = other_models(host, family).await;
    if models.is_empty() {
        host.notify_warning("pi lists no models of other providers with credentials.")
            .await?;
        return Ok(false);
    }
    let Some(choice) = host.select("Fall back to", &models).await? else {
        return Ok(false);
    };
    let Some(model) = ModelRef::parse(&choice) else {
        return Ok(false);
    };
    if policy.fallback.iter().any(|e| e.model == model) {
        return Ok(false);
    }
    policy.fallback.push(ChainEntry {
        model,
        thinking: None,
    });
    Ok(true)
}

/// `/router resets [<n>]`: banked usage resets of each Codex account; with a number, spends one.
pub async fn resets(host: &Host, state: &Arc<State>, args: &str) -> Result<()> {
    let ids = account_ids(state, Family::Codex);
    if let Some(slot) = args
        .split_whitespace()
        .next()
        .and_then(|w| w.parse::<u32>().ok())
    {
        if !ids.contains(&slot) {
            return host
                .notify_warning(format!("Codex has accounts {ids:?}."))
                .await;
        }
        let name = account_name(Family::Codex, slot);
        let question = format!(
            "Use one banked usage reset on {name}? It refreshes its five-hour and weekly windows."
        );
        if !host.confirm("Use a usage reset", &question).await? {
            return Ok(());
        }
        lock(&state.picks)
            .reset_tried
            .remove(&(Family::Codex, slot));
        if !crate::resets::try_reset(host, state, Family::Codex, slot).await {
            host.notify_warning(format!(
                "{name}: no usage reset was applied (none banked, or nothing to reset)."
            ))
            .await?;
        }
        crate::status::update(host, state).await;
        return Ok(());
    }
    let mut lines = Vec::new();
    for slot in &ids {
        let name = account_name(Family::Codex, *slot);
        match crate::resets::list(host, state, *slot).await {
            Ok(credits) => {
                lines.push(format!(
                    "{name}: {} reset(s) banked",
                    credits.available_count
                ));
                for credit in credits.credits.iter().filter(|c| c.status != "redeemed") {
                    let expires = credit
                        .expires_at
                        .as_deref()
                        .map_or(String::new(), |e| format!(", expires {e}"));
                    let title = credit
                        .title
                        .clone()
                        .unwrap_or_else(|| credit.reset_type.clone());
                    lines.push(format!("  {title} ({}{expires})", credit.status));
                }
            }
            Err(err) => lines.push(format!("{name}: {err}")),
        }
    }
    lines.push(
        "Anthropic saved resets are applied at claude.ai > Settings > Usage; pi cannot apply them."
            .into(),
    );
    lines.push("/router resets <n> spends one on Codex account n.".into());
    host.notify_info(lines.join("\n")).await
}

/// One line per provider for `/router status`: the policy and the account each uses now.
pub fn accounts_report(state: &State) -> Vec<String> {
    if !enabled(state) {
        return Vec::new();
    }
    let now = now_ms();
    let mut lines = Vec::new();
    for family in [Family::Anthropic, Family::Codex] {
        let ids = account_ids(state, family);
        if ids.len() < 2 {
            continue;
        }
        let policy = state.config().policy(family);
        let picks = lock(&state.picks);
        let current = picks.current.get(&family).copied().unwrap_or(1);
        let pinned = picks.pinned.get(&family).copied();
        drop(picks);
        let how = match pinned {
            Some(slot) => format!("pinned to #{slot}"),
            None => policy.strategy.as_str().to_owned(),
        };
        lines.push(format!(
            "{} accounts ({how}): using #{current}",
            family.label()
        ));
        for slot in ids {
            let full = lock(&state.picks)
                .full_until
                .get(&(family, slot))
                .copied()
                .filter(|u| *u > now);
            let note = full.map_or(String::new(), |u| {
                format!(" · full for {}", duration(u - now))
            });
            lines.push(format!("  {}{note}", account_line(state, family, slot)));
        }
    }
    if let Some(model) = current_fallback(state) {
        lines.push(format!(
            "Falling back to {model} until an account has room."
        ));
    }
    lines
}

fn current_fallback(state: &State) -> Option<String> {
    lock(&state.picks)
        .fallback_from
        .as_ref()
        .map(|from| from.to.key())
}
