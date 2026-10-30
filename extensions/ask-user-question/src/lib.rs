//! `ask_user_question`: a tabbed questionnaire panel for structured answers from the user.

mod answer;
pub mod channel;
pub mod config;
pub mod external;
mod fallback;
pub mod guidance;
mod keys;
mod layout;
mod panel;
pub mod render;
pub mod result;
pub mod schema;
pub mod state;
pub mod tool;

#[cfg(test)]
mod tests;

use std::process::ExitCode;
use std::sync::Arc;

use pi_ext_sdk::prelude::*;

use crate::channel::AnswerChannel;
use crate::config::Config;
use crate::guidance::{TOOL_NAME, tool_spec};

/// Strips the tool when pi has no UI and restores it when the UI returns.
async fn reconcile(host: &Host, has_ui: bool) -> Result<()> {
    let active = host.active_tools().await?;
    let has_tool = active.iter().any(|name| name == TOOL_NAME);
    if !has_ui && has_tool {
        let kept: Vec<&String> = active.iter().filter(|name| *name != TOOL_NAME).collect();
        host.set_active_tools(&kept).await?;
    } else if has_ui && !has_tool {
        let mut names = active.clone();
        names.push(TOOL_NAME.to_owned());
        host.set_active_tools(&names).await?;
    }
    Ok(())
}

/// The extension with `config` (guidance is read once, at registration).
pub fn extension(config: &Config) -> Extension {
    let channel = Arc::new(AnswerChannel::default());
    let start_channel = channel.clone();
    Extension::new("ask-user-question")
        .tool(
            tool_spec(&config.guidance),
            move |args: Value, ctx: ToolCtx| tool::execute(args, ctx, channel.clone()),
        )
        .on("before_agent_start", |ev: EventCtx| async move {
            reconcile(ev.host(), ev.snapshot().has_ui).await
        })
        .on_start(move |host: Host| {
            let channel = start_channel.clone();
            async move { channel.ensure_listening(&host).await }
        })
}

pub fn main() -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_default();
    let config = Config::load(&pi_ext_sdk::paths::agent_dir(), &cwd);
    extension(&config).run()
}
