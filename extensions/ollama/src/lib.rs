//! Ollama for pi-rust: discovers local models, registers provider `ollama` (OpenAI-compatible
//! API at `<host>/v1`), and manages models with `/ollama`.

pub mod client;
pub mod discovery;
pub mod panel;
pub mod state;
pub mod view;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use pi_ext_sdk::CompletionItem;
use pi_ext_sdk::prelude::*;

use client::Client;
use discovery::{ModelInfo, discover, provider_config};
use state::{Ollama, PROVIDER, global_host, resolve_host};

const SUBCOMMANDS: [&str; 6] = ["list", "refresh", "pull", "delete", "show", "host"];

/// Builds the extension. `initial` is a discovery result from before `initialize`, which
/// pi then knows at startup, so `--model ollama/<id>` resolves.
pub fn extension(env_host: Option<String>, initial: Option<(String, Vec<ModelInfo>)>) -> Extension {
    let declared = initial
        .as_ref()
        .filter(|(_, models)| models.iter().any(ModelInfo::can_chat))
        .map(|(host, models)| provider_config(host, models));
    let ollama = Arc::new(Ollama::new(env_host, initial));
    let mut ext = Extension::new("ollama");
    if let Some(config) = declared {
        ext = ext.provider(PROVIDER, config);
    }
    let start = ollama.clone();
    ext.on("session_start", move |ev: EventCtx| {
        let ollama = start.clone();
        async move {
            let host = ev.host().clone();
            tokio::spawn(async move {
                if let Err(error) = ollama.refresh(&host).await {
                    pi_ext_sdk::info!("ollama: discovery failed: {error}");
                }
            });
            Ok(())
        }
    })
    .command(
        "ollama",
        "Manage local Ollama models: list, pull, delete, refresh, host",
        move |args, ctx: CommandCtx| panel::run(ollama.clone(), args, ctx),
    )
    .completions("ollama", |prefix: String, _host: Host| async move {
        let items = SUBCOMMANDS
            .iter()
            .filter(|sub| sub.starts_with(prefix.trim()))
            .map(|sub| CompletionItem::new(*sub))
            .collect();
        Ok(Some(items))
    })
}

/// Discovery against the global host, bounded so pi does not wait on a stopped Ollama.
fn startup_discovery(env_host: Option<String>) -> Option<(String, Vec<ModelInfo>)> {
    let agent_dir = pi_ext_sdk::paths::agent_dir();
    let base = resolve_host(None, global_host(&agent_dir), env_host);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    runtime.block_on(async {
        let client = Client::new(&base);
        let models = tokio::time::timeout(Duration::from_millis(1500), discover(&client))
            .await
            .ok()?
            .ok()?;
        Some((base, models))
    })
}

pub fn main() -> ExitCode {
    let env_host = std::env::var("OLLAMA_HOST").ok();
    let initial = startup_discovery(env_host.clone());
    extension(env_host, initial).run()
}
