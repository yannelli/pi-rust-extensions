use std::sync::Arc;

use pi_ext_sdk::CompletionItem;
use pi_ext_sdk::prelude::*;

use crate::registry::{Registry, StartOptions};
use crate::task::Status;
use crate::tools::{logs_text, normalize_max_bytes, started_text};
use crate::{ensure, panel, ui};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BgArgs {
    pub name: Option<String>,
    pub is_agent: bool,
    pub survive_reload: bool,
    pub command: String,
}

/// Parses `/bg [--survive-reload|--no-survive-reload] [--agent] [--name "N"] <command>`.
pub fn parse_bg_args(input: &str) -> Result<BgArgs, String> {
    let mut args = BgArgs {
        name: None,
        is_agent: false,
        survive_reload: true,
        command: String::new(),
    };
    let mut rest = input.trim_start();
    loop {
        let (word, after) = split_word(rest);
        match word {
            "--survive-reload" => args.survive_reload = true,
            "--no-survive-reload" => args.survive_reload = false,
            "--agent" => args.is_agent = true,
            "--name" => {
                let (name, after) = take_value(after)?;
                args.name = Some(name);
                rest = after.trim_start();
                continue;
            }
            "--" => {
                rest = after.trim_start();
                break;
            }
            _ => break,
        }
        rest = after.trim_start();
    }
    if rest.trim().is_empty() {
        return Err(
            "usage: /bg [--no-survive-reload] [--agent] [--name \"Task name\"] <command>".into(),
        );
    }
    args.command = rest.trim().to_owned();
    Ok(args)
}

fn split_word(input: &str) -> (&str, &str) {
    let end = input.find(char::is_whitespace).unwrap_or(input.len());
    (&input[..end], &input[end..])
}

fn take_value(input: &str) -> Result<(String, &str), String> {
    let input = input.trim_start();
    let Some(quote) = input.chars().next().filter(|c| *c == '"' || *c == '\'') else {
        let (word, rest) = split_word(input);
        if word.is_empty() {
            return Err("--name needs a value".into());
        }
        return Ok((word.to_owned(), rest));
    };
    let body = &input[1..];
    let end = body
        .find(quote)
        .ok_or_else(|| format!("unclosed {quote} in --name"))?;
    Ok((body[..end].to_owned(), &body[end + 1..]))
}

async fn report(host: &Host, text: impl Into<String>, error: bool) -> Result<()> {
    let text = text.into();
    let sent = if error {
        host.notify_error(text.clone()).await
    } else {
        host.notify_info(text.clone()).await
    };
    if let Err(err) = sent {
        pi_ext_sdk::info!("{text} ({err})");
    }
    Ok(())
}

fn task_completions(
    registry: &Registry,
    prefix: &str,
    running_only: bool,
) -> Option<Vec<CompletionItem>> {
    let prefix = prefix.trim();
    let items: Vec<CompletionItem> = registry
        .tasks()
        .into_iter()
        .filter(|t| t.id.starts_with(prefix) && (!running_only || t.status == Status::Running))
        .take(20)
        .map(|t| {
            CompletionItem::new(t.id.clone())
                .label(format!("{} {}", t.id, t.name))
                .description(format!("{} — {}", t.status.label(), t.command))
        })
        .collect();
    (!items.is_empty()).then_some(items)
}

pub fn register(ext: Extension, registry: Arc<Registry>) -> Extension {
    let r = registry;
    let (bg, tasks, tasks2, jobs, logs, kill, clear) = (
        r.clone(),
        r.clone(),
        r.clone(),
        r.clone(),
        r.clone(),
        r.clone(),
        r.clone(),
    );
    let (logs_c, kill_c) = (r.clone(), r);
    ext.command(
        "bg",
        "Start a tracked shell command: /bg [--no-survive-reload] [--agent] [--name \"Task name\"] <command>",
        move |args, ctx: CommandCtx| {
            let registry = bg.clone();
            async move {
                ensure(ctx.host(), ctx.snapshot(), &registry);
                let parsed = match parse_bg_args(&args) {
                    Ok(parsed) => parsed,
                    Err(err) => return report(ctx.host(), err, true).await,
                };
                let options = StartOptions {
                    name: parsed.name,
                    command: parsed.command,
                    is_agent: parsed.is_agent,
                    survive_reload: parsed.survive_reload,
                    notify_on_completion: true,
                    trigger_on_completion: false,
                    ..StartOptions::default()
                };
                match registry.start(options, &ctx.snapshot().cwd) {
                    Ok(task) => {
                        let _ = ui::refresh(ctx.host(), &registry).await;
                        report(ctx.host(), started_text(&task), false).await
                    }
                    Err(err) => report(ctx.host(), format!("Background task failed to start: {err}"), true).await,
                }
            }
        },
    )
    .command("tasks", "Open the background task manager", move |args, ctx: CommandCtx| {
        let registry = tasks.clone();
        async move {
            ensure(ctx.host(), ctx.snapshot(), &registry);
            panel::open(ctx.host().clone(), registry, args.trim().to_owned()).await
        }
    })
    .command("bg-tasks", "Open the background task manager", move |args, ctx: CommandCtx| {
        let registry = tasks2.clone();
        async move {
            ensure(ctx.host(), ctx.snapshot(), &registry);
            panel::open(ctx.host().clone(), registry, args.trim().to_owned()).await
        }
    })
    .command("jobs", "List running and recent background tasks", move |_args, ctx: CommandCtx| {
        let registry = jobs.clone();
        async move {
            ensure(ctx.host(), ctx.snapshot(), &registry);
            report(ctx.host(), ui::list_text(&registry.tasks()), false).await
        }
    })
    .command("logs", "Show bounded output from a background task: /logs <id> [maxBytes]", move |args, ctx: CommandCtx| {
        let registry = logs.clone();
        async move {
            ensure(ctx.host(), ctx.snapshot(), &registry);
            let mut words = args.split_whitespace();
            let id = words.next().unwrap_or_default();
            let max = normalize_max_bytes(words.next().and_then(|w| w.parse().ok()));
            let text = registry
                .resolve(id)
                .and_then(|task| logs_text(&task, max, true).map_err(|e| e.to_string()));
            match text {
                Ok(text) => report(ctx.host(), text, false).await,
                Err(err) => report(ctx.host(), format!("Background logs error: {err}"), true).await,
            }
        }
    })
    .completions("logs", move |prefix, _host| {
        let registry = logs_c.clone();
        async move { Ok(task_completions(&registry, &prefix, false)) }
    })
    .command("kill", "Stop a running background task: /kill <id>", move |args, ctx: CommandCtx| {
        let registry = kill.clone();
        async move {
            ensure(ctx.host(), ctx.snapshot(), &registry);
            match registry.stop(&args, "stopped by /kill").await {
                Ok(task) => {
                    let _ = ui::refresh(ctx.host(), &registry).await;
                    let text = format!("Killed {} ({}). Output: {}", task.name, task.id, task.output_path().display());
                    report(ctx.host(), text, false).await
                }
                Err(err) => report(ctx.host(), format!("Background kill error: {err}"), true).await,
            }
        }
    })
    .completions("kill", move |prefix, _host| {
        let registry = kill_c.clone();
        async move { Ok(task_completions(&registry, &prefix, true)) }
    })
    .command("bg-clear", "Remove finished background tasks and their output", move |_args, ctx: CommandCtx| {
        let registry = clear.clone();
        async move {
            ensure(ctx.host(), ctx.snapshot(), &registry);
            let removed = registry.clear_finished();
            let _ = ui::refresh(ctx.host(), &registry).await;
            report(ctx.host(), format!("Removed {removed} finished background task(s)."), false).await
        }
    })
}
