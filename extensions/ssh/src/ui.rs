use pi_ext_sdk::prelude::*;

use crate::{State, command, config, process};

pub async fn status(state: &State, host: &Host) -> Result<()> {
    let name = state.remote.lock().await.clone();
    let text = if let Some(name) = name {
        let config = config::load(host, &state.warned).await?;
        Some(match command::profile(&config, &name) {
            Ok(profile) => format!("SSH: {name} ({})", profile.host),
            Err(_) => format!("SSH: {name} (unavailable; bash blocked)"),
        })
    } else {
        None
    };
    host.set_status("ssh", text.as_deref()).await
}

async fn remote(state: &State, host: &Host, name: Option<String>) -> Result<()> {
    if let Some(name) = &name {
        command::profile(&config::load(host, &state.warned).await?, name)?;
    }
    host.append_entry("ssh-remote", &name).await?;
    *state.remote.lock().await = name;
    status(state, host).await
}

async fn connection(state: &State, ctx: &CommandCtx, name: &str, action: &str) -> Result<()> {
    let host = ctx.host();
    let config = config::load(host, &state.warned).await?;
    let profile = config
        .profiles
        .get(name)
        .ok_or_else(|| Error::msg("Unknown SSH profile"))?;
    if action != "disconnect" {
        command::profile(&config, name)?;
    }
    let mut process = if action == "disconnect" {
        let mut process = tokio::process::Command::new("ssh");
        process
            .current_dir(host.agent_dir().join("ext/ssh/sockets"))
            .args(command::arguments(&host.agent_dir(), &config, profile)?)
            .args(["-O", "exit"])
            .arg(&profile.host);
        process
    } else {
        command::build(&host.agent_dir(), &config, profile, "true")?
    };
    process.stdin(std::process::Stdio::null());
    let output = process::run(process, None, 20, ctx.cancel_token().clone(), None).await?;
    if output.code != Some(0) {
        return Err(Error::msg(format!(
            "SSH {action} failed: {}",
            output.stderr
        )));
    }
    host.notify_info(format!("SSH {action}: {name}")).await
}

async fn scope(host: &Host) -> Result<Option<std::path::PathBuf>> {
    let Some(scope) = host.select("Config scope", &["Global", "Project"]).await? else {
        return Ok(None);
    };
    let (global, project) = config::paths(host);
    if scope == "Project" {
        if !config::trusted(host).await? {
            return Err(Error::msg("Project is not trusted"));
        }
        Ok(Some(project))
    } else {
        Ok(Some(global))
    }
}

async fn edit(state: &State, host: &Host, existing: Option<&str>) -> Result<()> {
    let Some(path) = scope(host).await? else {
        return Ok(());
    };
    let name = match existing {
        Some(name) => name.to_owned(),
        None => match host.input("Profile name", Some("production")).await? {
            Some(name) if !name.trim().is_empty() => name.trim().to_owned(),
            _ => return Ok(()),
        },
    };
    let config = config::load(host, &state.warned).await?;
    let value = config
        .profiles
        .get(&name)
        .map(serde_json::to_value)
        .transpose()?
        .unwrap_or_else(|| {
            json!({
                "host":"", "user":null, "port":22, "identityFile":null, "jumpHost":null,
                "cwd":null, "env":{}, "options":{}, "enabled":true
            })
        });
    let prefill = serde_json::to_string_pretty(&value)?;
    let Some(text) = host.editor("SSH profile", Some(&prefill)).await? else {
        return Ok(());
    };
    let profile: config::Profile = serde_json::from_str(&text)?;
    config::validate(&name, &profile)?;
    let mut document = config::read(&path)?;
    document["profiles"][&name] = serde_json::to_value(profile)?;
    config::save(&path, &document)?;
    host.notify_info(format!("Saved SSH profile: {name}")).await
}

async fn modify(state: &State, host: &Host, name: &str, delete: bool) -> Result<()> {
    let Some(path) = scope(host).await? else {
        return Ok(());
    };
    let mut document = config::read(&path)?;
    if delete {
        if !host
            .confirm(
                "Delete SSH profile",
                &format!("Delete {name} from {}?", path.display()),
            )
            .await?
        {
            return Ok(());
        }
        if let Some(profiles) = document.get_mut("profiles").and_then(Value::as_object_mut) {
            profiles.remove(name);
        }
    } else {
        let config = config::load(host, &state.warned).await?;
        let profile = config
            .profiles
            .get(name)
            .ok_or_else(|| Error::msg("Unknown SSH profile"))?;
        let mut profile = profile.clone();
        profile.enabled = !profile.enabled;
        document["profiles"][name] = serde_json::to_value(profile)?;
    }
    config::save(&path, &document)?;
    status(state, host).await
}

pub async fn manage(state: &State, args: &str, ctx: CommandCtx) -> Result<()> {
    let _edit = state.edits.lock().await;
    let host = ctx.host();
    let (action, name) = args.trim().split_once(' ').unwrap_or((args.trim(), ""));
    match action {
        "off" => return remote(state, host, None).await,
        "remote" => return remote(state, host, Some(name.trim().into())).await,
        "connect" | "disconnect" | "test" => {
            return connection(state, &ctx, name.trim(), action).await;
        }
        "" => {}
        _ => {
            return Err(Error::msg(
                "Usage: /ssh [remote <profile>|off|connect <profile>|disconnect <profile>|test <profile>]",
            ));
        }
    }
    loop {
        let config = config::load(host, &state.warned).await?;
        let names: Vec<&String> = config.profiles.keys().collect();
        let mut options = vec!["Add profile".to_owned(), "Turn remote mode off".into()];
        options.extend(config.profiles.iter().map(|(name, p)| {
            format!(
                "{name} ({}){}",
                p.host,
                if p.enabled { "" } else { " [disabled]" }
            )
        }));
        let Some(choice) = host.select("SSH profiles", &options).await? else {
            break;
        };
        let Some(index) = options.iter().position(|s| *s == choice) else {
            continue;
        };
        match index {
            0 => edit(state, host, None).await?,
            1 => remote(state, host, None).await?,
            _ => {
                let name = names[index - 2];
                let actions = [
                    "Edit",
                    "Test",
                    "Connect",
                    "Disconnect",
                    "Use for bash",
                    "Enable or disable",
                    "Delete",
                ];
                let Some(action) = host.select(name, &actions).await? else {
                    continue;
                };
                match action.as_str() {
                    "Edit" => edit(state, host, Some(name)).await?,
                    "Test" => connection(state, &ctx, name, "test").await?,
                    "Connect" => connection(state, &ctx, name, "connect").await?,
                    "Disconnect" => connection(state, &ctx, name, "disconnect").await?,
                    "Use for bash" => remote(state, host, Some(name.clone())).await?,
                    "Enable or disable" => modify(state, host, name, false).await?,
                    "Delete" => modify(state, host, name, true).await?,
                    _ => {}
                }
            }
        }
    }
    Ok(())
}
