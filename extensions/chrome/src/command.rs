use std::sync::Arc;
use std::time::SystemTime;

use pi_ext_sdk::{CompletionItem, Host, Result};

use crate::auth::{self, Grant};
use crate::manage;
use crate::session;
use crate::state::Chrome;

const SUBCOMMANDS: &[(&str, &str)] = &[
    ("authorize", "Allow Chrome control for this session"),
    (
        "revoke",
        "Lock the chrome tools and close this session's tabs",
    ),
    ("doctor", "Show connection, config, and authorization state"),
    ("background", "on, off, toggle, or status"),
    ("launch", "Connect to Chrome, or start the managed profile"),
    ("config", "Edit global or project settings"),
];

pub async fn run(chrome: Arc<Chrome>, args: String, host: Host) -> Result<()> {
    let mut words = args.split_whitespace();
    let head = words.next().unwrap_or("");
    let rest = words.collect::<Vec<_>>().join(" ");
    match head {
        "" => menu(&chrome, &host).await,
        "authorize" => authorize(&chrome, &host, &rest).await,
        "revoke" => {
            session::lock(&chrome, &host, "revoked").await?;
            host.notify_info("Chrome control revoked.").await
        }
        "doctor" => host.notify_info(manage::doctor(&chrome, &host).await).await,
        "background" => background(&chrome, &host, &rest).await,
        "launch" => launch(&chrome, &host).await,
        "config" => manage::config_menu(&chrome, &host).await,
        other => {
            let text = format!(
                "Unknown subcommand '{other}'. Use /chrome authorize | revoke | doctor | background | launch | config."
            );
            host.notify_warning(text).await
        }
    }
}

pub async fn complete(prefix: String) -> Result<Option<Vec<CompletionItem>>> {
    let (head, partial) = match prefix.split_once(' ') {
        Some((head, partial)) => (head, partial.trim_start()),
        None => ("", prefix.as_str()),
    };
    let candidates: Vec<(String, &str)> = match head {
        "" => SUBCOMMANDS
            .iter()
            .map(|(n, d)| ((*n).to_owned(), *d))
            .collect(),
        "authorize" => ["15m", "30m", "60m", "indefinite"]
            .iter()
            .map(|v| (format!("authorize {v}"), "Duration"))
            .collect(),
        "background" => ["on", "off", "toggle", "status"]
            .iter()
            .map(|v| (format!("background {v}"), "Background mode"))
            .collect(),
        _ => return Ok(None),
    };
    let items: Vec<CompletionItem> = candidates
        .into_iter()
        .filter(|(value, _)| {
            value
                .rsplit(' ')
                .next()
                .unwrap_or(value)
                .starts_with(partial)
        })
        .map(|(value, description)| {
            let label = value.rsplit(' ').next().unwrap_or(&value).to_owned();
            CompletionItem::new(value.clone())
                .label(label)
                .description(description)
        })
        .collect();
    Ok(Some(items).filter(|items| !items.is_empty()))
}

async fn menu(chrome: &Arc<Chrome>, host: &Host) -> Result<()> {
    if !host.has_ui() {
        return host.notify_info(manage::doctor(chrome, host).await).await;
    }
    let state = chrome
        .grant
        .lock()
        .expect("grant")
        .describe(SystemTime::now());
    let title = format!("Chrome: {state}");
    let options: Vec<String> = SUBCOMMANDS
        .iter()
        .map(|(n, d)| format!("{n}: {d}"))
        .collect();
    let Some(choice) = host.select(&title, &options).await? else {
        return Ok(());
    };
    let name = choice.split(':').next().unwrap_or_default();
    match name {
        "authorize" => authorize_menu(chrome, host).await,
        "background" => background(chrome, host, "toggle").await,
        other => Box::pin(run(chrome.clone(), other.to_owned(), host.clone())).await,
    }
}

async fn authorize_menu(chrome: &Arc<Chrome>, host: &Host) -> Result<()> {
    let options = [
        "15 minutes",
        "30 minutes",
        "60 minutes",
        "Until revoked",
        "Custom minutes",
    ];
    let arg = match host
        .select("Authorize Chrome for", &options)
        .await?
        .as_deref()
    {
        Some("15 minutes") => "15m".to_owned(),
        Some("30 minutes") => "30m".to_owned(),
        Some("60 minutes") => "60m".to_owned(),
        Some("Until revoked") => "indefinite".to_owned(),
        Some(_) => match host.input("Minutes", Some("45")).await? {
            Some(minutes) => minutes,
            None => return Ok(()),
        },
        None => return Ok(()),
    };
    authorize(chrome, host, &arg).await
}

async fn authorize(chrome: &Arc<Chrome>, host: &Host, arg: &str) -> Result<()> {
    if !host.has_ui() {
        return host
            .notify_error("/chrome authorize needs the interactive UI to confirm.")
            .await;
    }
    let config = chrome.config(host).await?;
    let duration = match auth::parse_duration(arg, config.default_authorize_minutes) {
        Ok(duration) => duration,
        Err(err) => return host.notify_warning(err.to_string()).await,
    };
    let span = duration.map_or("until you revoke it".to_owned(), |d| {
        format!("for {}", auth::countdown(d))
    });
    let message = format!(
        "pi can open tabs, read pages, click, type, and run JavaScript in the Chrome on {}:{} {span}. \
         Sites where that profile is signed in are included.",
        config.host, config.port
    );
    if !host.confirm("Allow Chrome control?", &message).await? {
        return host.notify_info("Chrome control not changed.").await;
    }
    let action = if chrome.authorized() {
        "reauthorized"
    } else {
        "authorized"
    };
    let grant = Grant::for_duration(duration, SystemTime::now());
    session::grant(chrome, host, grant, action).await?;
    host.notify_info(format!("Chrome control {action} {span}."))
        .await
}

async fn background(chrome: &Chrome, host: &Host, arg: &str) -> Result<()> {
    let current = chrome.background(host).await;
    let next = match arg.trim() {
        "on" => true,
        "off" => false,
        "toggle" => !current,
        "" | "status" => current,
        other => {
            return host
                .notify_warning(format!(
                    "Unknown option '{other}'. Use on, off, toggle, or status."
                ))
                .await;
        }
    };
    chrome.set_background(Some(next));
    let text = if next {
        "Background mode is on: new tabs open in the background and tools do not focus tabs."
    } else {
        "Background mode is off: new tabs open in the front, and background=false focuses a tab."
    };
    host.notify_info(text).await
}

async fn launch(chrome: &Chrome, host: &Host) -> Result<()> {
    match chrome.ensure(host, true).await {
        Ok(_) => host.notify_info(manage::doctor(chrome, host).await).await,
        Err(err) => {
            host.notify_error(format!("Chrome launch failed: {err:#}"))
                .await
        }
    }
}
