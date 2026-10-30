use std::time::SystemTime;

use pi_ext_sdk::{Host, Result};

use crate::browser;
use crate::config::{self, KEYS};
use crate::state::Chrome;

pub async fn doctor(chrome: &Chrome, host: &Host) -> String {
    let mut lines = Vec::new();
    let global = config::global_path(&host.agent_dir());
    let project = config::project_path(&host.cwd());
    let trusted = Chrome::trusted(host).await;
    lines.push(format!("Global config: {}", global.display()));
    let project_state = match (project.exists(), trusted) {
        (false, _) => "not present",
        (true, true) => "used",
        (true, false) => "ignored, the project is not trusted",
    };
    lines.push(format!(
        "Project config: {} ({project_state})",
        project.display()
    ));
    match chrome.reload_config(host).await {
        Ok(config) => {
            lines.push(format!(
                "Mode: {}, endpoint {}:{}",
                config.mode_name(),
                config.host,
                config.port
            ));
            lines.push(format!(
                "Managed profile: {}",
                config.user_data_dir.display()
            ));
            match browser::discover(&config.host, config.port).await {
                Ok((ws, version)) => {
                    let product = version["Browser"].as_str().unwrap_or("Chrome");
                    let managed = browser::launched_here(&Chrome::state_dir(host), &ws, false);
                    lines.push(format!(
                        "DevTools: reachable, {product}{}",
                        if managed { " (managed)" } else { "" }
                    ));
                }
                Err(err) => lines.push(format!("DevTools: not reachable ({err:#})")),
            }
        }
        Err(err) => lines.push(format!("Config error: {err:#}")),
    }
    let grant = chrome
        .grant
        .lock()
        .expect("grant")
        .describe(SystemTime::now());
    lines.push(format!("Authorization: {grant}"));
    lines.push(format!(
        "Background mode: {}",
        if chrome.background(host).await {
            "on"
        } else {
            "off"
        }
    ));
    lines.push(format!("Session tabs: {}", chrome.owned().len()));
    lines.join("\n")
}

pub async fn config_menu(chrome: &Chrome, host: &Host) -> Result<()> {
    if !host.has_ui() {
        return host
            .notify_error("/chrome config needs the interactive UI.")
            .await;
    }
    let scopes = ["Global", "Project"];
    let Some(scope) = host.select("Chrome config scope", &scopes).await? else {
        return Ok(());
    };
    let path = match scope.as_str() {
        "Project" => config::project_path(&host.cwd()),
        _ => config::global_path(&host.agent_dir()),
    };
    loop {
        let file = config::read_file(&path)?;
        let current = serde_json::to_value(&file).unwrap_or_default();
        let options: Vec<String> = KEYS
            .iter()
            .map(
                |(key, help)| match current.get(*key).filter(|v| !v.is_null()) {
                    Some(value) => format!("{key} = {value}  ({help})"),
                    None => format!("{key} (unset)  ({help})"),
                },
            )
            .collect();
        let title = format!("{scope} config: {}", path.display());
        let Some(choice) = host.select(&title, &options).await? else {
            break;
        };
        let key = choice
            .split([' ', '='])
            .next()
            .unwrap_or_default()
            .to_owned();
        let Some(input) = host
            .input(&format!("{key} (empty removes it)"), None)
            .await?
        else {
            continue;
        };
        let write = config::parse_value(&key, &input)
            .and_then(|value| config::write_key(&path, &key, value));
        match write {
            Ok(()) => {
                host.notify_info(format!("Saved {key} in {}", path.display()))
                    .await?
            }
            Err(err) => host.notify_warning(format!("{err:#}")).await?,
        }
    }
    chrome.reload_config(host).await?;
    chrome.disconnect().await;
    Ok(())
}
