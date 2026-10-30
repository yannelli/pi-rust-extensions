//! A configurable footer status line for pi-rust: builtin presets, `command` scripts, and the
//! status lines of Claude Code, Cursor, and Codex.

pub mod builtin;
pub mod chips;
pub mod clock;
pub mod colors;
pub mod command;
pub mod config;
pub mod editor;
pub mod editor_items;
pub mod external;
pub mod footer;
pub mod icons;
pub mod items;
pub mod layout;
pub mod list;
pub mod model;
pub mod panel;
pub mod presets;
pub mod segments;
pub mod sources;
pub mod themes;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use pi_ext_sdk::CompletionItem;
use pi_ext_sdk::prelude::*;

use footer::Footer;

const RENDER_EVENTS: &[&str] = &[
    "session_start",
    "session_tree",
    "session_before_compact",
    "session_compact",
    "agent_start",
    "agent_end",
    "turn_start",
    "turn_end",
    "message_start",
    "message_end",
    "tool_execution_start",
    "tool_execution_end",
    "model_select",
];

/// Ticks the clock segment and `refreshInterval` of command status lines.
async fn ticker(footer: Arc<Footer>, host: Host) {
    let mut seconds = 0u64;
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        seconds += 1;
        match footer.has_refresh() {
            Some(period) if seconds % period.as_secs().max(1) == 0 => {
                footer.refresh_command(&host).await
            }
            _ if seconds % 30 == 0 => footer.render(&host).await,
            _ => {}
        }
    }
}

pub fn extension(home: PathBuf) -> Extension {
    let footer = Footer::new(home, String::new());
    let mut ext = Extension::new("statusline");
    for event in RENDER_EVENTS {
        let footer = footer.clone();
        ext = ext.on(event, move |ev: EventCtx| {
            let footer = footer.clone();
            async move {
                footer.observe(&ev);
                if ev.name() == "session_start" {
                    footer.reload(ev.host()).await;
                } else {
                    footer.render(ev.host()).await;
                }
                Ok(())
            }
        });
    }
    let (start, surface, command) = (footer.clone(), footer.clone(), footer.clone());
    ext.on_start(move |host: Host| {
        let footer = start.clone();
        async move {
            footer.set_version(host.init().map(|init| init.pi_version).unwrap_or_default());
            footer.find_project(&host).await;
            footer.reload(&host).await;
            segments::usage::start(&host);
            tokio::spawn(ticker(footer, host));
            Ok(())
        }
    })
    .on_surface(move |event: SurfaceEvent, host: Host| {
        let footer = surface.clone();
        async move {
            match event {
                SurfaceEvent::Resize { surface, .. } if surface == "footer" => {
                    footer.render(&host).await
                }
                SurfaceEvent::Closed { surface } if surface == "footer" => footer.forget_sent(),
                _ => {}
            }
            Ok(())
        }
    })
    .command(
        "statusline",
        "Edit or switch the footer status line",
        move |args, ctx: CommandCtx| panel::statusline_command(command.clone(), args, ctx),
    )
    .completions("statusline", |prefix: String, _host: Host| async move {
        let mut choices: Vec<String> = presets::preset_names().map(str::to_string).collect();
        choices.extend(
            [
                "edit",
                "command",
                "from-claude",
                "from-cursor",
                "from-codex",
                "off",
            ]
            .map(String::from),
        );
        let items = choices
            .into_iter()
            .filter(|choice| choice.starts_with(prefix.trim()))
            .map(CompletionItem::new)
            .collect();
        Ok(Some(items))
    })
}

pub fn main() -> ExitCode {
    colors::init_mode(
        std::env::var("COLORTERM").ok().as_deref(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
    );
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    extension(home).run()
}
