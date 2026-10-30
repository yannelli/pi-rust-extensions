use std::sync::Arc;

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::{CancellationToken, CompletionItem, NotifyType};

use crate::backends::{BACKEND_ENV, NAMES, backend};
use crate::client::{Client, DEFAULT_MAX_REQUESTS};
use crate::credentials::{KeySituation, normalize_key};
use crate::errors::{TsError, TsResult};
use crate::result::format_result;
use crate::state::State;
use crate::status::status_text;
use crate::{notify, prompt, sample, validate};

const ACTIONS: [&str; 8] = [
    "login",
    "logout",
    "setup",
    "status",
    "enable",
    "disable",
    "test",
    "playground",
];

pub fn complete(prefix: &str) -> Option<Vec<CompletionItem>> {
    let (verb, rest) = match prefix.split_once(char::is_whitespace) {
        Some((verb, rest)) => (verb, Some(rest.split_whitespace().collect::<Vec<_>>())),
        None => (prefix, None),
    };
    let item = |value: String, label: &str| CompletionItem {
        value,
        label: Some(label.to_owned()),
        description: None,
    };
    let backend_rest = rest.filter(|_| matches!(verb, "login" | "logout"));
    let items: Vec<CompletionItem> = if let Some(rest) = backend_rest {
        let typed = rest.join(" ");
        NAMES
            .iter()
            .filter(|n| n.starts_with(&typed))
            .map(|n| item(format!("{verb} {n}"), n))
            .collect()
    } else {
        ACTIONS
            .iter()
            .filter(|a| a.starts_with(prefix))
            .map(|a| item((*a).to_owned(), a))
            .collect()
    };
    (!items.is_empty()).then_some(items)
}

async fn login(state: &State, host: &Host, spec: &crate::backends::Backend) -> Result<()> {
    if state.env.get(spec.key_env).is_some() {
        let text = format!(
            "{} is set in the environment and takes precedence over a stored key. Unset it before using /typesafe login.",
            spec.key_env
        );
        notify(host, &text, NotifyType::Warning).await;
        return Ok(());
    }
    let Some(entered) = prompt::prompt_for_key(host, spec.label).await? else {
        notify(
            host,
            "Login cancelled; nothing was saved.",
            NotifyType::Info,
        )
        .await;
        return Ok(());
    };
    let key = normalize_key(&entered)?;
    let target = match spec.name == state.backend.name {
        true => state.backend.clone(),
        false => spec.clone(),
    };
    let client = Client::new(
        target,
        Some(key.clone()),
        None,
        &state.dir,
        &state.env,
        Default::default(),
        state.clock.clone(),
    )?;
    let cancel = CancellationToken::new();
    let models = match spec.models_verify_key {
        true => Some(client.list_models(&cancel).await?.len()),
        false => {
            let probe = json!({ "state": "login", "questions": { "key": { "type": "noul", "instructions": "Is this key accepted?" } } });
            client.evaluate(probe, &cancel).await?;
            None
        }
    };
    let path = state.keys().store(&key, spec)?;
    state.drop_client();
    let proof = match models {
        None => "verified with one request".to_owned(),
        Some(n) => format!(
            "verified ({n} model{} available)",
            if n == 1 { "" } else { "s" }
        ),
    };
    let hint = match (spec.key_env == state.backend.key_env, state.enabled()) {
        (true, true) => String::new(),
        (true, false) => " Run /typesafe enable to allow agent tool calls.".to_owned(),
        (false, _) => format!(
            " {BACKEND_ENV} is {}; set it to {} to send judgments there.",
            state.backend.name, spec.name
        ),
    };
    let text = format!(
        "{} key {proof} and saved to {} with owner-only permissions.{hint}",
        spec.label,
        path.display()
    );
    notify(host, &text, NotifyType::Info).await;
    Ok(())
}

async fn send_request(state: &State, host: &Host, action: &str) -> TsResult<Option<Value>> {
    let mut request = sample();
    if action == "playground" {
        let prefill = serde_json::to_string_pretty(&request).unwrap_or_default();
        let edited = host
            .editor(
                "TypeSafe request JSON · edit state and questions",
                Some(&prefill),
            )
            .await;
        let Some(text) = edited.map_err(|e| TsError::configuration(e.message()))? else {
            return Ok(None);
        };
        let Ok(parsed) = serde_json::from_str(&text) else {
            notify(
                host,
                "Invalid JSON. Keep quoted strings on one line; nothing was sent.",
                NotifyType::Error,
            )
            .await;
            return Ok(None);
        };
        request = parsed;
    }
    let validated = validate::prepare(request, None)?;
    let (consent, _) = state.consent();
    let confirmed = host.confirm("Send this TypeSafe request?", &consent).await;
    if !confirmed.map_err(|e| TsError::configuration(e.message()))? {
        return Ok(None);
    }
    state
        .get_client()?
        .evaluate(validated, &CancellationToken::new())
        .await
        .map(Some)
}

async fn act(state: &State, host: &Host, action: &str, backend_arg: Option<&str>) -> Result<()> {
    let spec = backend(backend_arg.unwrap_or(state.backend.name))?;
    let is_active = spec.key_env == state.backend.key_env;
    let situation = state.keys().situation(&state.backend, &state.env);
    let report = |text: String, level| async move { notify(host, &text, level).await };
    match action {
        "status" => {
            let (text, level) = status_text(state);
            report(text, level).await;
            return Ok(());
        }
        "logout" => {
            let removed = state.keys().clear(spec.name);
            state.auth().clear();
            state.drop_client();
            let env = state.env.get(spec.key_env);
            if is_active && env.is_none() {
                state.set_enabled(false);
            }
            let tail = match (&env, is_active) {
                (Some(_), _) => format!(
                    " {} is still set in the environment and takes precedence over a stored key.",
                    spec.key_env
                ),
                (None, true) => " Judgments are off for this session.".to_owned(),
                (None, false) => String::new(),
            };
            let head = match removed {
                true => format!(
                    "Removed the stored {} key at {}.",
                    spec.label,
                    state.keys().path.display()
                ),
                false => format!("No stored {} key to remove.", spec.label),
            };
            report(format!("{head}{tail}"), NotifyType::Info).await;
            return Ok(());
        }
        "disable" => {
            state.set_enabled(false);
            let text =
                "TypeSafe disabled for future agent calls. In-flight requests are not cancelled.";
            report(text.to_owned(), NotifyType::Info).await;
            return Ok(());
        }
        _ if !ACTIONS.contains(&action) => {
            let text = format!(
                "Usage: /typesafe {}. login and logout take a backend name: {}.",
                ACTIONS.join(" | "),
                NAMES.join(", ")
            );
            report(text, NotifyType::Warning).await;
            return Ok(());
        }
        _ if !host.has_ui() => {
            let text = format!(
                "This command needs interactive Pi. For headless tool use, explicitly set PI_TYPESAFE_ENABLED=1 and {} before launching Pi.",
                spec.key_env
            );
            report(text, NotifyType::Warning).await;
            return Ok(());
        }
        _ => {}
    }
    if action == "login" || (action == "setup" && situation == KeySituation::Missing) {
        return login(state, host, &spec).await;
    }
    match (action, &situation) {
        ("setup", _) => {
            let current = match &situation {
                KeySituation::Unusable { reason } => format!("unusable — {reason}"),
                other => format!("configured via {}", other.label()),
            };
            let text = format!(
                "Key {current}. Run /typesafe test for one sample request or /typesafe enable to allow agent tool calls."
            );
            report(text, NotifyType::Info).await;
        }
        ("enable", KeySituation::Missing) => {
            report(
                "Run /typesafe login first: no API key is configured.".to_owned(),
                NotifyType::Warning,
            )
            .await
        }
        ("enable", KeySituation::Unusable { reason }) => {
            report(
                format!("The stored key cannot be used. {reason}"),
                NotifyType::Warning,
            )
            .await
        }
        ("enable", _) => {
            let (consent, _) = state.consent();
            if host
                .confirm("Enable TypeSafe for this session?", &consent)
                .await?
            {
                state.set_enabled(true);
                let text = format!(
                    "TypeSafe enabled. Up to {DEFAULT_MAX_REQUESTS} attempts in this session; /typesafe disable stops future agent calls."
                );
                report(text, NotifyType::Info).await;
            }
        }
        _ => {
            if let Some(result) = send_request(state, host, action).await? {
                host.append_entry("typesafe-result", &result).await?;
                report(format_result(&result, true), NotifyType::Info).await;
            }
        }
    }
    Ok(())
}

pub async fn run(state: Arc<State>, args: String, ctx: CommandCtx) -> Result<()> {
    let host = ctx.host().clone();
    let mut words = args.split_whitespace();
    let action = words.next().unwrap_or("status");
    let backend_arg = words.next();
    if let (Some(error), "login" | "logout") = (&state.backend_error, action) {
        let text = format!(
            "{error} No key change fixes that, so /typesafe {action} does nothing: fix or unset {BACKEND_ENV} first."
        );
        notify(&host, &text, NotifyType::Warning).await;
        return Ok(());
    }
    if let Err(error) = act(&state, &host, action, backend_arg).await {
        notify(&host, error.message(), NotifyType::Error).await;
    }
    Ok(())
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
