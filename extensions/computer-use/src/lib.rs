use std::future::Future;
use std::process::ExitCode;
use std::sync::Arc;

use pi_ext_sdk::prelude::*;
use serde::de::DeserializeOwned;

mod act;
mod backend;
mod browser;
mod cdp;
mod cond;
mod config;
mod live;
mod outline;
mod output;
mod page;
mod page_act;
mod platform;
mod query;
mod runtime;
mod schema;
mod setup;
mod state;
mod tools;

#[cfg(test)]
mod tests;

use live::Live;
use runtime::{Runtime, Shared};

pub use query::unavailable as state_unavailable_message;
pub use state::TOOL_NAMES;

/// Upstream tools throw; here every failure becomes an `isError` result with the same text.
fn finish(result: Result<ToolResult>) -> Result<ToolResult> {
    Ok(result.unwrap_or_else(|err| ToolResult::error(err.message())))
}

fn cached<A, F>(ext: Extension, rt: &Shared, spec: ToolSpec, run: F) -> Extension
where
    A: DeserializeOwned + Send + 'static,
    F: Fn(&Runtime, A) -> Result<ToolResult> + Copy + Send + Sync + 'static,
{
    let rt = rt.clone();
    ext.tool(spec, move |args: A, _ctx: ToolCtx| {
        let rt = rt.clone();
        async move { finish(run(&rt, args)) }
    })
}

fn live<A, F, Fut>(ext: Extension, rt: &Shared, spec: ToolSpec, run: F) -> Extension
where
    A: DeserializeOwned + Send + 'static,
    F: Fn(Shared, ToolCtx, A) -> Fut + Copy + Send + Sync + 'static,
    Fut: Future<Output = Result<ToolResult>> + Send + 'static,
{
    let rt = rt.clone();
    ext.tool(spec, move |args: A, ctx: ToolCtx| {
        let rt = rt.clone();
        async move { finish(run(rt, ctx, args).await) }
    })
}

macro_rules! live_tool {
    ($method:ident) => {
        |rt: Shared, ctx: ToolCtx, args| async move {
            let _guard = rt.live.lock().await;
            let live = Live {
                host: ctx.host(),
                rt: &rt,
                cancel: ctx.cancel_token(),
            };
            live.$method(args).await
        }
    };
}

async fn replay(rt: Shared, ev: EventCtx) -> Result<()> {
    let host = ev.host();
    rt.reload(host, Some(ev.snapshot())).await;
    let entries = host.branch(None).await.unwrap_or_default();
    rt.store().restore_from_branch(&entries);
    Ok(())
}

async fn command(rt: Shared, args: String, ctx: CommandCtx) -> Result<()> {
    let host = ctx.host();
    let loaded = rt.reload(host, Some(ctx.snapshot())).await;
    if args.trim() != "setup" {
        return host.notify_info(config::format_status(&loaded)).await;
    }
    let _guard = rt.live.lock().await;
    match setup::ensure_ready(host, ctx.cancel_token()).await {
        Ok(_) => host.notify_info("pi-computer-use is ready.").await,
        Err(err) => host.notify_warning(err.message()).await,
    }
}

pub fn extension() -> Extension {
    let rt: Shared = Arc::new(Runtime::default());
    let mut ext = Extension::new("computer-use");
    ext = live(ext, &rt, schema::find_roots(), live_tool!(find_roots));
    ext = live(ext, &rt, schema::observe_ui(), live_tool!(observe));
    ext = cached(ext, &rt, schema::search_ui(), query::search);
    ext = cached(ext, &rt, schema::expand_ui(), query::expand);
    ext = cached(ext, &rt, schema::inspect_ui(), query::inspect);
    ext = live(ext, &rt, schema::act_ui(), live_tool!(act));
    ext = cached(ext, &rt, schema::read_text(), query::read_text);
    ext = live(ext, &rt, schema::wait_for(), live_tool!(wait_for));
    ext = live(
        ext,
        &rt,
        schema::launch_browser(),
        live_tool!(launch_browser),
    );
    ext = live(
        ext,
        &rt,
        schema::navigate_browser(),
        live_tool!(navigate_browser),
    );
    ext = live(
        ext,
        &rt,
        schema::evaluate_browser(),
        live_tool!(evaluate_browser),
    );
    for event in ["session_start", "session_tree", "session_compact"] {
        let rt = rt.clone();
        ext = ext.on(event, move |ev: EventCtx| replay(rt.clone(), ev));
    }
    let command_rt = rt.clone();
    ext.command(
        "computer-use",
        "Show pi-computer-use configuration; `/computer-use setup` checks tools and permissions",
        move |args, ctx| command(command_rt.clone(), args, ctx),
    )
    .on_shutdown(move |_host: Host| {
        let rt = rt.clone();
        async move {
            rt.browser().take();
            Ok(())
        }
    })
}

pub fn main() -> ExitCode {
    extension().run()
}
