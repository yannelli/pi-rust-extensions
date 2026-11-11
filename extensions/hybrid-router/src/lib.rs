//! hybrid-router: the virtual model `hybrid/auto`. The classifier of a local model sends simple
//! prompts to the local model and the others to a remote model; busy machines skip the classifier.

pub mod config;
pub mod route;
pub mod setup;

use std::process::ExitCode;
use std::sync::Arc;

use pi_ext_sdk::CompletionItem;
use pi_ext_sdk::prelude::*;

use route::Router;

const SUBCOMMANDS: [&str; 2] = ["status", "off"];

pub fn main() -> ExitCode {
    let saved = config::load(&pi_ext_sdk::paths::agent_dir()).unwrap_or_else(|error| {
        pi_ext_sdk::warn!("hybrid-router: ignoring config. {error}");
        None
    });
    extension(Arc::new(Router::new(saved, route::load_per_core))).run()
}

/// The extension. With a config, `hybrid/auto` is declared at `initialize`, so
/// `--model hybrid/auto` resolves at startup.
pub fn extension(router: Arc<Router>) -> Extension {
    let mut ext = Extension::new("hybrid-router");
    if router.config().is_some() {
        let routes = router.clone();
        ext = ext.virtual_model(route::definition(), move |request, host| {
            route::route(routes.clone(), request, host)
        });
    }
    ext.command(
        "hybrid",
        "Route prompts between a local and a remote model (hybrid/auto): setup, status, off",
        move |args, ctx: CommandCtx| setup::run(router.clone(), args, ctx),
    )
    .completions("hybrid", |prefix: String, _host: Host| async move {
        let items = SUBCOMMANDS
            .iter()
            .filter(|sub| sub.starts_with(prefix.trim()))
            .map(|sub| CompletionItem::new(*sub))
            .collect();
        Ok(Some(items))
    })
}
