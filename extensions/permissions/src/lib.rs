mod ask;
mod codex;
mod codex_config;
mod evaluate;
mod file_rules;
mod files;
mod listing;
mod load;
mod mode;
mod paths;
mod pattern;
mod requirements;
mod rules;
mod settings;
mod shell;
mod ui;
mod wrappers;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use pi_ext_sdk::CompletionItem;
use pi_ext_sdk::event_types::ToolCallEvent;
use pi_ext_sdk::prelude::*;

pub use paths::Paths;

use evaluate::{Call, evaluate};
use load::{Env, load};
use mode::{Mode, Outcome};
use rules::Decision;

fn env_for<'a>(paths: &'a Paths, host: &Host, snap: &Snapshot) -> Env<'a> {
    Env {
        paths,
        agent_dir: paths.agent_dir.clone().unwrap_or_else(|| host.agent_dir()),
        cwd: PathBuf::from(&snap.cwd),
        trusted: snap.is_project_trusted,
    }
}

async fn tool_call(paths: Arc<Paths>, ev: EventCtx) -> Result<EventReply> {
    let event: ToolCallEvent = ev.event_as()?;
    let (host, snap) = (ev.host(), ev.snapshot());
    let env = env_for(&paths, host, snap);
    let loaded = load(&env);
    if let Some(reason) = loaded.blocked() {
        return Ok(EventReply::block(reason));
    }
    let call = Call {
        tool: &event.tool_name,
        input: &event.input,
        cwd: &env.cwd,
        home: &paths.home,
        dirs: &loaded.dirs,
    };
    let verdict = evaluate(&loaded.rules, &call);
    let mode = loaded.mode.as_ref();
    let dont_ask = mode.is_some_and(|(m, _)| *m == Mode::DontAsk);
    match (verdict.decision, &verdict.rule) {
        (Some(Decision::Deny), Some(rule)) => Ok(EventReply::block(ask::deny_reason(rule))),
        (Some(Decision::Ask), Some(rule)) if dont_ask => Ok(EventReply::block(format!(
            "{} Claude dontAsk mode denies calls that would ask.",
            ask::deny_reason(rule)
        ))),
        (Some(Decision::Ask), _) => ask::ask(host, &loaded, &verdict, &call, None).await,
        (Some(Decision::Allow), _) => Ok(EventReply::none()),
        _ => match mode.map(|(m, label)| (mode::decide(*m, &call, &verdict), label)) {
            Some((Outcome::Ask, label)) => {
                ask::ask(host, &loaded, &verdict, &call, Some(label)).await
            }
            Some((Outcome::Deny(reason), _)) => Ok(EventReply::block(reason)),
            _ => Ok(EventReply::none()),
        },
    }
}

async fn session_start(paths: Arc<Paths>, ev: EventCtx) -> Result<EventReply> {
    let loaded = load(&env_for(&paths, ev.host(), ev.snapshot()));
    for note in &loaded.notes {
        pi_ext_sdk::warn!("permissions: {note}");
    }
    Ok(EventReply::none())
}

pub fn extension(paths: Paths) -> Extension {
    let paths = Arc::new(paths);
    let (call_paths, start_paths, command_paths) = (paths.clone(), paths.clone(), paths);
    Extension::new("permissions")
        .on("tool_call", move |ev: EventCtx| {
            tool_call(call_paths.clone(), ev)
        })
        .on("session_start", move |ev: EventCtx| {
            session_start(start_paths.clone(), ev)
        })
        .command(
            "permissions",
            "Allow, ask, and deny rules from Claude Code, Codex, and pi",
            move |args, ctx: CommandCtx| ui::command(command_paths.clone(), args, ctx),
        )
        .completions("permissions", |prefix: String, _host: Host| async move {
            Ok(Some(
                ["list", "test "]
                    .iter()
                    .filter(|c| c.starts_with(&prefix))
                    .map(|c| CompletionItem::new(*c))
                    .collect(),
            ))
        })
}

pub fn main() -> ExitCode {
    extension(Paths::from_env()).run()
}
