//! The user config of an installed plugin, asked with host dialogs (outside the panel).

use pi_ext_sdk::InputOptions;
use pi_ext_sdk::prelude::*;

use crate::env::Env;
use crate::{blocking, install, options};

/// Asks for the user config of an installed plugin: every option with `all`, else the ones
/// without a stored value. Saves the answers and reinstalls; `None` when nothing was asked.
pub async fn prompt_options(
    host: &Host,
    env: &Env,
    id: &str,
    all: bool,
) -> anyhow::Result<Option<String>> {
    let key = id.to_owned();
    let (scope, opts, stored) = blocking(env.clone(), move |env| {
        install::user_options(&env, &key, None)
    })
    .await?;
    let asked: Vec<&options::Opt> = opts
        .iter()
        .filter(|opt| all || !stored.contains_key(&opt.key))
        .collect();
    if asked.is_empty() {
        return Ok(None);
    }
    let mut updates = Vec::new();
    for opt in asked {
        let current = stored.get(&opt.key).or(opt.default.as_ref());
        let prefill = current.filter(|_| !opt.sensitive).map(options::to_text);
        let mut title = format!("{id}: {}", opt.title);
        if opt.required {
            title.push_str(" (required)");
        }
        let input = InputOptions {
            placeholder: Some(opt.description.clone()).filter(|d| !d.is_empty()),
            prefill,
            secret: opt.sensitive,
        };
        let answer = host
            .input_with(&title, input)
            .await
            .map_err(Error::into_anyhow)?;
        let Some(answer) = answer else { break };
        if !answer.trim().is_empty() {
            updates.push((opt.key.clone(), Some(answer)));
        }
    }
    if updates.is_empty() {
        return Ok(None);
    }
    let key = id.to_owned();
    let text = blocking(env.clone(), move |env| {
        install::configure(&env, &key, Some(scope), &updates)
    })
    .await?;
    Ok(Some(text))
}
