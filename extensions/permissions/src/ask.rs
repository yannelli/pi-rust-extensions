//! The dialog for calls that an ask rule matches, and the deny reason the model sees.

use pi_ext_sdk::prelude::*;

use crate::evaluate::{Call, Verdict, suggestions};
use crate::load::Loaded;
use crate::rules::{Decision, Rule};
use crate::settings::{self, Scope};

pub const ALLOW_ONCE: &str = "Allow once";
pub const ALLOW_ALWAYS: &str = "Allow always";
pub const DENY: &str = "Deny";
pub const DENY_REASON: &str = "Deny with a reason";
pub const USER_DENIED: &str = "The user denied this tool call.";

pub fn deny_reason(rule: &Rule) -> String {
    let mut reason = format!(
        "Blocked by permission rule {} from {} ({}).",
        rule.text,
        rule.source.label(),
        rule.file.display()
    );
    if let Some(justification) = &rule.justification {
        reason.push(' ');
        reason.push_str(justification);
    }
    reason
}

fn summary(call: &Call) -> String {
    let detail = ["command", "path", "url"]
        .iter()
        .find_map(|key| call.input.get(*key).and_then(Value::as_str))
        .map(str::to_owned)
        .unwrap_or_else(|| call.input.to_string());
    let detail: String = detail.chars().take(400).collect();
    format!("{}: {detail}", call.tool)
}

/// Asks about a call that an ask rule matched, or that `mode` (a Claude `defaultMode` label)
/// asks about. Only a mode ask offers "Allow always": an ask rule wins over every allow rule.
pub async fn ask(
    host: &Host,
    loaded: &Loaded,
    verdict: &Verdict,
    call: &Call<'_>,
    mode: Option<&str>,
) -> Result<EventReply> {
    let asked_by = match (&verdict.rule, mode) {
        (Some(rule), _) if rule.decision == Decision::Ask => {
            format!("permission rule {}", rule.describe())
        }
        (_, Some(mode)) => format!("Claude defaultMode {mode}"),
        _ => return Ok(EventReply::none()),
    };
    if !host.has_ui() {
        return Ok(EventReply::block(format!(
            "The {asked_by} asks before this call, and there is no UI to ask the user."
        )));
    }
    let title = format!("Allow this call?\n{}\nAsked by {asked_by}", summary(call));
    let options: &[&str] = if mode.is_some() && verdict.decision.is_none() {
        &[ALLOW_ONCE, ALLOW_ALWAYS, DENY, DENY_REASON]
    } else {
        &[ALLOW_ONCE, DENY, DENY_REASON]
    };
    let choice = host.select(&title, options).await?;
    if choice.as_deref() == Some(ALLOW_ALWAYS) {
        let rules = suggestions(verdict, call.tool);
        if save_always(host, loaded, &rules).await? {
            return Ok(EventReply::none());
        }
        return Ok(EventReply::block(USER_DENIED));
    }
    answer(host, choice.as_deref()).await
}

/// Asks about a write to a Claude protected path, without "Allow always": an allow rule would not apply.
pub async fn ask_protected(host: &Host, call: &Call<'_>, reason: &str) -> Result<EventReply> {
    if !host.has_ui() {
        return Ok(EventReply::block(format!(
            "Claude asks before this call: {reason}. There is no UI to ask the user."
        )));
    }
    let title = format!(
        "Allow this call?\n{}\nAsked because {reason}",
        summary(call)
    );
    let choice = host
        .select(&title, &[ALLOW_ONCE, DENY, DENY_REASON])
        .await?;
    answer(host, choice.as_deref()).await
}

async fn answer(host: &Host, choice: Option<&str>) -> Result<EventReply> {
    match choice {
        Some(ALLOW_ONCE) => Ok(EventReply::none()),
        Some(DENY_REASON) => {
            let reason = host.input("Reason for the model", None).await?;
            Ok(EventReply::block(
                match reason.filter(|r| !r.trim().is_empty()) {
                    Some(reason) => format!("The user denied this tool call: {}", reason.trim()),
                    None => USER_DENIED.to_owned(),
                },
            ))
        }
        _ => Ok(EventReply::block(USER_DENIED)),
    }
}

/// Asks where to save `rules` as allow rules; `false` when the user closed the dialog.
async fn save_always(host: &Host, loaded: &Loaded, rules: &[String]) -> Result<bool> {
    let mut scopes = vec![loaded.options.scope, loaded.options.scope.other()];
    if !loaded.trusted {
        scopes.retain(|s| *s == Scope::Global);
    }
    let label = |scope: Scope| {
        let file = match scope {
            Scope::Global => &loaded.global_file,
            Scope::Project => &loaded.project_file,
        };
        format!("Save to {} ({})", scope.name(), file.display())
    };
    let options: Vec<String> = scopes.iter().map(|s| label(*s)).collect();
    let title = format!("Allow always: {}", rules.join(", "));
    let Some(choice) = host.select(&title, &options).await? else {
        return Ok(false);
    };
    let Some(scope) = scopes.into_iter().find(|s| label(*s) == choice) else {
        return Ok(false);
    };
    let file = match scope {
        Scope::Global => &loaded.global_file,
        Scope::Project => &loaded.project_file,
    };
    let saved = settings::edit(file, |f| {
        for rule in rules {
            f.add(Decision::Allow, rule);
        }
    });
    if let Err(err) = saved {
        let _ = host.notify_error(format!("permissions: {err}")).await;
    }
    Ok(true)
}
