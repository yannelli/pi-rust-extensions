//! The `/permissions` command and panel.

use std::sync::Arc;

use pi_ext_sdk::prelude::*;

use crate::evaluate::{Call, sample_call};
use crate::listing::{explain, listing, on_off, rows};
use crate::load::{Loaded, load};
use crate::rules::{Decision, Rule, Source, split_claude};
use crate::settings::{self, Scope};
use crate::{Paths, env_for};

enum Action {
    Add,
    Remove(Box<Rule>),
    Sources,
    Scope,
    Test,
}

pub async fn command(paths: Arc<Paths>, args: String, ctx: CommandCtx) -> Result<()> {
    let (host, snap) = (ctx.host(), ctx.snapshot());
    let reload = || load(&env_for(&paths, host, snap));
    let args = args.trim();
    if let Some(sample) = args.strip_prefix("test ") {
        return test(&paths, host, &reload(), sample).await;
    }
    if args == "list" || !host.has_ui() {
        let loaded = reload();
        return host
            .notify_info(listing(&loaded, &rows(&loaded), None).join("\n"))
            .await;
    }
    let mut selected = 0usize;
    loop {
        let loaded = reload();
        let rules = rows(&loaded);
        selected = selected.min(rules.len().saturating_sub(1));
        let mut panel = host
            .panel(listing(&loaded, &rules, Some(selected)))
            .open()
            .await?;
        let action = loop {
            let key = match panel.next_event().await {
                PanelEvent::Key(key) => key,
                PanelEvent::Closed => return Ok(()),
                PanelEvent::Resize(_) => {
                    panel.set_lines(listing(&loaded, &rules, Some(selected)))?;
                    continue;
                }
            };
            match key.char() {
                _ if key.is_cancel() => break None,
                _ if key.is("up") => selected = selected.saturating_sub(1),
                _ if key.is("down") => selected = (selected + 1).min(rules.len().saturating_sub(1)),
                Some('q') => break None,
                Some('a') => break Some(Action::Add),
                Some('d') if !rules.is_empty() => {
                    break Some(Action::Remove(Box::new(rules[selected].clone())));
                }
                Some('s') => break Some(Action::Sources),
                Some('c') => break Some(Action::Scope),
                Some('t') => break Some(Action::Test),
                _ => {}
            }
            panel.set_lines(listing(&loaded, &rules, Some(selected)))?;
        };
        let _ = panel.close(Value::Null).await;
        let Some(action) = action else {
            return Ok(());
        };
        run(&paths, host, &loaded, action).await?;
    }
}

async fn run(paths: &Paths, host: &Host, loaded: &Loaded, action: Action) -> Result<()> {
    let result = match action {
        Action::Add => add(host, loaded).await?,
        Action::Remove(rule) if rule.source.is_pi() => settings::edit(&rule.file, |f| {
            f.remove(&rule.text);
        }),
        Action::Remove(rule) => Err(format!(
            "{} rules are read-only here; edit {}",
            rule.source.label(),
            rule.file.display()
        )),
        Action::Sources => {
            let keys = Source::toggle_keys();
            let labels: Vec<String> = keys
                .iter()
                .map(|k| format!("{k}: {}", on_off(loaded.options.key_on(k))))
                .collect();
            let title = format!(
                "Turn a source on or off ({} file)",
                loaded.options.scope.name()
            );
            match host.select(&title, &labels).await? {
                Some(choice) => {
                    let key = choice.split(':').next().unwrap_or_default().to_owned();
                    let on = !loaded.options.key_on(&key);
                    match scope_file(loaded, loaded.options.scope) {
                        Ok(file) => settings::edit(file, |f| {
                            f.sources.insert(key, on);
                        }),
                        Err(err) => Err(err),
                    }
                }
                None => Ok(()),
            }
        }
        Action::Scope => set_scope(loaded, loaded.options.scope.other()),
        Action::Test => {
            if let Some(sample) = host
                .input("Command or Tool(argument) to test", None)
                .await?
            {
                test(paths, host, loaded, &sample).await?;
            }
            Ok(())
        }
    };
    if let Err(err) = result {
        host.notify_error(format!("permissions: {err}")).await?;
    }
    Ok(())
}

async fn add(host: &Host, loaded: &Loaded) -> Result<std::result::Result<(), String>> {
    let Some(text) = host
        .input("New rule, for example Bash(git push *)", None)
        .await?
    else {
        return Ok(Ok(()));
    };
    let text = text.trim().to_owned();
    if split_claude(&text).is_none() {
        return Ok(Err(format!("cannot read rule {text:?}")));
    }
    let names: Vec<&str> = Decision::ALL.iter().map(|d| d.name()).collect();
    let Some(choice) = host.select(&format!("Decision for {text}"), &names).await? else {
        return Ok(Ok(()));
    };
    let Some(decision) = Decision::parse(&choice) else {
        return Ok(Ok(()));
    };
    Ok(scope_file(loaded, loaded.options.scope)
        .and_then(|file| settings::edit(file, |f| f.add(decision, &text))))
}

fn scope_file(loaded: &Loaded, scope: Scope) -> std::result::Result<&std::path::Path, String> {
    match scope {
        Scope::Global => Ok(&loaded.global_file),
        Scope::Project if loaded.trusted => Ok(&loaded.project_file),
        Scope::Project => Err("the project is not trusted; project settings are not used".into()),
    }
}

/// Writes `scope` to the global file and drops a project override.
pub fn set_scope(loaded: &Loaded, scope: Scope) -> std::result::Result<(), String> {
    settings::edit(&loaded.global_file, |f| f.scope = Some(scope))?;
    if loaded.trusted && loaded.project_file.exists() {
        settings::edit(&loaded.project_file, |f| f.scope = None)?;
    }
    Ok(())
}

async fn test(paths: &Paths, host: &Host, loaded: &Loaded, sample: &str) -> Result<()> {
    let (tool, input) = sample_call(sample);
    let call = Call {
        tool: &tool,
        input: &input,
        cwd: &loaded.cwd,
        home: &paths.home,
        dirs: &loaded.dirs,
    };
    host.notify_info(explain(loaded, &call).join("\n")).await
}
