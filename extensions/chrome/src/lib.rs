mod actions;
mod auth;
mod browser;
mod cdp;
mod command;
mod config;
mod input;
mod keys;
mod manage;
mod page;
mod raw;
mod screenshot;
mod session;
mod snapshot;
mod specs;
mod specs_input;
mod state;
mod tabs;
mod tools;

use std::future::Future;
use std::process::ExitCode;
use std::sync::Arc;

use pi_ext_sdk::event_types::{SessionShutdownEvent, SessionStartEvent};
use pi_ext_sdk::prelude::*;
use serde::de::DeserializeOwned;

use actions::Action;
use state::Chrome;

pub use auth::NOT_AUTHORIZED;
pub use specs::TOOL_NAMES;

/// Locked sessions answer every tool with an error before any network access.
fn guarded<A, F, Fut>(ext: Extension, chrome: &Arc<Chrome>, spec: ToolSpec, run: F) -> Extension
where
    A: DeserializeOwned + Send + 'static,
    F: Fn(Arc<Chrome>, A, ToolCtx) -> Fut + Send + Sync + Clone + 'static,
    Fut: Future<Output = anyhow::Result<ToolResult>> + Send + 'static,
{
    let chrome = chrome.clone();
    ext.tool(spec, move |args: A, ctx: ToolCtx| {
        let (chrome, run) = (chrome.clone(), run.clone());
        async move {
            if !chrome.authorized() {
                return Ok(ToolResult::error(NOT_AUTHORIZED));
            }
            let cancel = ctx.cancel_token().clone();
            tokio::select! {
                result = run(chrome, args, ctx) => {
                    Ok(result.unwrap_or_else(|err| ToolResult::error(format!("{err:#}"))))
                }
                () = cancel.cancelled() => Err(Error::msg("cancelled")),
            }
        }
    })
}

type ActionFuture = std::pin::Pin<Box<dyn Future<Output = anyhow::Result<ToolResult>> + Send>>;

fn action(
    kind: Action,
) -> impl Fn(Arc<Chrome>, actions::ActionArgs, ToolCtx) -> ActionFuture + Send + Sync + Clone + 'static
{
    move |chrome, args, ctx| Box::pin(actions::run(chrome, kind, args, ctx))
}

fn with_tools(mut ext: Extension, chrome: &Arc<Chrome>) -> Extension {
    for spec in specs::specs() {
        ext = match spec.name.as_str() {
            "chrome_launch" => guarded(ext, chrome, spec, tabs::launch),
            "chrome_tab" => guarded(ext, chrome, spec, tabs::tab),
            "chrome_snapshot" => guarded(ext, chrome, spec, tools::snapshot),
            "chrome_find" => guarded(ext, chrome, spec, tools::find),
            "chrome_navigate" => guarded(ext, chrome, spec, tools::navigate),
            "chrome_evaluate" => guarded(ext, chrome, spec, tools::evaluate),
            "chrome_wait_for" => guarded(ext, chrome, spec, tools::wait_for),
            "chrome_click" => guarded(ext, chrome, spec, action(Action::Click)),
            "chrome_type" => guarded(ext, chrome, spec, action(Action::Type)),
            "chrome_fill" => guarded(ext, chrome, spec, action(Action::Fill)),
            "chrome_key" => guarded(ext, chrome, spec, action(Action::Key)),
            "chrome_hover" => guarded(ext, chrome, spec, action(Action::Hover)),
            "chrome_scroll" => guarded(ext, chrome, spec, action(Action::Scroll)),
            "chrome_upload_file" => guarded(ext, chrome, spec, action(Action::Upload)),
            "chrome_screenshot" => guarded(ext, chrome, spec, screenshot::run),
            "chrome_cdp" => guarded(ext, chrome, spec, raw::cdp),
            "chrome_cdp_targets" => guarded(ext, chrome, spec, raw::targets),
            other => unreachable!("no handler for {other}"),
        };
    }
    ext
}

pub fn extension() -> Extension {
    let chrome = Arc::new(Chrome::default());
    let ext = with_tools(Extension::new("chrome"), &chrome);
    let (on_start, on_stop, on_prompt, on_command) =
        (chrome.clone(), chrome.clone(), chrome.clone(), chrome);
    ext.command(
        "chrome",
        "Authorize and manage Chrome control",
        move |args, ctx: CommandCtx| command::run(on_command.clone(), args, ctx.host().clone()),
    )
    .completions("chrome", |prefix, _host| command::complete(prefix))
    .on("session_start", move |ev: EventCtx| {
        let chrome = on_start.clone();
        async move {
            let reason = ev.event_as::<SessionStartEvent>()?.reason;
            session::start(&chrome, ev.host(), &reason).await
        }
    })
    .on("before_agent_start", move |_ev: EventCtx| {
        let authorized = on_prompt.authorized();
        async move {
            let primer = authorized.then(|| session::PRIMER.to_owned());
            Ok(EventReply::none().with_section(session::PRIMER_KEY, primer))
        }
    })
    .on("session_shutdown", move |ev: EventCtx| {
        let chrome = on_stop.clone();
        async move {
            let reason = ev.event_as::<SessionShutdownEvent>()?.reason;
            let stop = session::shutdown(&chrome, ev.host(), &reason);
            let _ = tokio::time::timeout(std::time::Duration::from_millis(1_500), stop).await;
            Ok(())
        }
    })
}

pub fn main() -> ExitCode {
    extension().run()
}
