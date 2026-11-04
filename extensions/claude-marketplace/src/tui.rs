use pi_ext_sdk::InputOptions;
use pi_ext_sdk::prelude::*;

use crate::env::{Env, Scope};
use crate::listing::{self, MarketplaceRow, PluginRow, Status};
use crate::{blocking, install, market, options, reconcile};

enum Row {
    Marketplace(MarketplaceRow),
    Plugin(PluginRow),
}

impl Row {
    fn marketplace(&self) -> &str {
        match self {
            Row::Marketplace(row) => &row.name,
            Row::Plugin(row) => &row.marketplace,
        }
    }
}

enum Action {
    Menu,
    Add,
    Update,
    Remove,
}

fn rows(env: &Env) -> anyhow::Result<Vec<Row>> {
    let marketplaces = listing::marketplaces(env, None)?;
    let plugins = listing::plugins(env, None, None)?;
    let mut out = Vec::new();
    for mp in marketplaces {
        let found = plugins
            .iter()
            .find(|p| p.name == mp.name && p.scope == mp.scope);
        let children: Vec<PluginRow> = found.map(|p| p.plugins.clone()).unwrap_or_default();
        out.push(Row::Marketplace(mp));
        out.extend(children.into_iter().map(Row::Plugin));
    }
    Ok(out)
}

fn render(rows: &[Row], cursor: usize, width: Option<usize>) -> Vec<String> {
    let mut lines = vec![
        style::bold("Claude plugins"),
        style::dim("up/down move, enter actions, a add marketplace, u update, d remove, q close"),
    ];
    if rows.is_empty() {
        lines.push("No marketplaces. Press a to add one.".into());
    }
    for (index, row) in rows.iter().enumerate() {
        let mark = if index == cursor { "> " } else { "  " };
        let text = match row {
            Row::Marketplace(mp) => style::bold(&listing::marketplace_line(mp)),
            Row::Plugin(p) => {
                let mut text = format!("{} [{}]", listing::plugin_line(p), p.scope);
                if let Some(description) = &p.description {
                    text.push_str(&format!(" - {}", style::dim(description)));
                }
                text
            }
        };
        let line = format!("{mark}{text}");
        lines.push(match width {
            Some(width) => style::truncate(&line, width),
            None => line,
        });
    }
    lines
}

/// The `/claude-plugins` browser. Dialogs run after the panel closes; it then opens again.
pub async fn browse(host: Host, env: Env) -> Result<()> {
    let mut cursor = 0usize;
    loop {
        let all = blocking(env.clone(), |env| rows(&env)).await?;
        cursor = cursor.min(all.len().saturating_sub(1));
        let mut panel = host.panel(render(&all, cursor, None)).open().await?;
        let action = loop {
            match panel.next_event().await {
                PanelEvent::Key(key) if key.is_cancel() => break None,
                PanelEvent::Key(key) if key.is("up") => cursor = cursor.saturating_sub(1),
                PanelEvent::Key(key) if key.is("down") => {
                    cursor = (cursor + 1).min(all.len().saturating_sub(1))
                }
                PanelEvent::Key(key) if key.is("enter") && !all.is_empty() => {
                    break Some(Action::Menu);
                }
                PanelEvent::Key(key) if key.is("a") => break Some(Action::Add),
                PanelEvent::Key(key) if key.is("u") && !all.is_empty() => {
                    break Some(Action::Update);
                }
                PanelEvent::Key(key) if key.is("d") && !all.is_empty() => {
                    break Some(Action::Remove);
                }
                PanelEvent::Key(_) | PanelEvent::Resize(_) => {}
                PanelEvent::Closed => return Ok(()),
            }
            panel.set_lines(render(&all, cursor, panel.width()))?;
        };
        panel.close(Value::Null).await?;
        let Some(action) = action else {
            return Ok(());
        };
        let message = act(&host, &env, action, all.get(cursor)).await;
        match message {
            Ok(Some(text)) => host.notify_info(text).await?,
            Ok(None) => {}
            Err(err) => host.notify_error(format!("{err:#}")).await?,
        }
    }
}

async fn run<F>(env: &Env, op: F) -> anyhow::Result<Option<String>>
where
    F: FnOnce(Env) -> anyhow::Result<String> + Send + 'static,
{
    Ok(Some(blocking(env.clone(), op).await?))
}

async fn act(
    host: &Host,
    env: &Env,
    action: Action,
    row: Option<&Row>,
) -> anyhow::Result<Option<String>> {
    match (action, row) {
        (Action::Add, _) => {
            let source = host
                .input(
                    "Marketplace source",
                    Some("owner/repo, git URL, or local path"),
                )
                .await
                .map_err(Error::into_anyhow)?;
            let Some(source) = source.filter(|s| !s.trim().is_empty()) else {
                return Ok(None);
            };
            let Some(scope) = pick_scope(host).await? else {
                return Ok(None);
            };
            run(env, move |env| market::add(&env, &source, scope)).await
        }
        (Action::Update, Some(row)) => update_marketplace(env, row.marketplace()).await,
        (Action::Remove, Some(row)) => remove_marketplace(host, env, row.marketplace()).await,
        (Action::Menu, Some(Row::Marketplace(mp))) => {
            let options = ["Update marketplace", "Remove marketplace", "Update all"];
            let choice = host
                .select(&mp.name, &options)
                .await
                .map_err(Error::into_anyhow)?;
            match choice.as_deref() {
                Some("Update marketplace") => update_marketplace(env, &mp.name).await,
                Some("Remove marketplace") => remove_marketplace(host, env, &mp.name).await,
                Some("Update all") => run(env, |env| reconcile::update_all(&env)).await,
                _ => Ok(None),
            }
        }
        (Action::Menu, Some(Row::Plugin(p))) => plugin_menu(host, env, p).await,
        _ => Ok(None),
    }
}

async fn update_marketplace(env: &Env, name: &str) -> anyhow::Result<Option<String>> {
    let name = name.to_owned();
    run(env, move |env| {
        let located = market::find(&env, &name, None)?;
        let mut lines = market::update(&env, &located)?;
        lines.push(crate::RELOAD.into());
        Ok(lines.join("\n"))
    })
    .await
}

async fn remove_marketplace(host: &Host, env: &Env, name: &str) -> anyhow::Result<Option<String>> {
    let message = format!("Remove marketplace {name} and uninstall its plugins?");
    if !host
        .confirm("Remove marketplace", &message)
        .await
        .map_err(Error::into_anyhow)?
    {
        return Ok(None);
    }
    let name = name.to_owned();
    run(env, move |env| market::remove(&env, &name, None)).await
}

async fn pick_scope(host: &Host) -> anyhow::Result<Option<Scope>> {
    let choice = host
        .select("Scope", &["user", "project"])
        .await
        .map_err(Error::into_anyhow)?;
    choice.map(|c| Scope::parse(&c)).transpose()
}

async fn plugin_menu(host: &Host, env: &Env, p: &PluginRow) -> anyhow::Result<Option<String>> {
    let id = p.id();
    let mut options = Vec::new();
    match p.enabled {
        None if p.status != Status::Unavailable => {
            options.extend(["Install (user)", "Install (project)"])
        }
        None => {}
        Some(enabled) => {
            options.extend(["Update", "Uninstall"]);
            options.push(if enabled { "Disable" } else { "Enable" });
        }
    }
    if p.enabled.is_some() {
        options.push("Configure");
    }
    options.push("Info");
    let Some(choice) = host
        .select(&id, &options)
        .await
        .map_err(Error::into_anyhow)?
    else {
        return Ok(None);
    };
    let scope = p.scope;
    if choice == "Configure" {
        let text = prompt_options(host, env, &id, true).await?;
        return Ok(text.or_else(|| Some(format!("{id} has no user config."))));
    }
    let installs = choice.starts_with("Install") || choice == "Enable";
    let (env2, id2) = (env.clone(), id.clone());
    let text = run(env, move |env| match choice.as_str() {
        "Install (user)" => install::install(&env, &id, Scope::User),
        "Install (project)" => install::install(&env, &id, Scope::Project),
        "Update" => install::update_one(&env, &id, Some(scope)),
        "Uninstall" => install::uninstall(&env, &id, Some(scope)),
        "Enable" => install::set_enabled(&env, &id, true, Some(scope)),
        "Disable" => install::set_enabled(&env, &id, false, Some(scope)),
        _ => crate::cli::info(&env, &id),
    })
    .await?;
    if installs {
        if let Some(configured) = prompt_options(host, &env2, &id2, false).await? {
            return Ok(Some(configured));
        }
    }
    Ok(text)
}

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
