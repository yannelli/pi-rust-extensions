//! A small native extension. Try it with a scripted transcript:
//! `cargo run -p pi-ext-sdk --example hello < transcript.jsonl`

use std::process::ExitCode;

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::style::{self, Color};
use serde::Deserialize;

#[derive(Deserialize)]
struct GreetArgs {
    name: String,
}

fn widget_lines(width: usize) -> Vec<String> {
    let line = format!(
        "{} {}",
        style::bold("hello"),
        style::dim("extension loaded")
    );
    vec![style::truncate(&line, width)]
}

async fn hello_panel(host: Host) -> Result<()> {
    let mut panel = host
        .panel(["Hello panel", "Press q or Esc to close"])
        .open()
        .await?;
    loop {
        match panel.next_event().await {
            PanelEvent::Key(key) if key.is_cancel() => break,
            PanelEvent::Key(key) => {
                panel.set_lines(["Hello panel".to_owned(), format!("Key: {key:?}")])?
            }
            PanelEvent::Resize(width) => {
                panel.set_lines([style::truncate("Hello panel, q closes", width)])?
            }
            PanelEvent::Closed => return Ok(()),
        }
    }
    panel.close("closed").await?;
    Ok(())
}

pub fn main() -> ExitCode {
    let greet = ToolSpec::new("greet", "Greet someone").parameters(json!({
        "type": "object",
        "properties": { "name": { "type": "string" } },
        "required": ["name"]
    }));
    Extension::new("hello")
        .tool(greet, |args: GreetArgs, ctx: ToolCtx| async move {
            ctx.update("working")?;
            Ok(format!("Hello, {}", args.name))
        })
        .command("hello", "Say hello", |_args, ctx: CommandCtx| async move {
            let host = ctx.host();
            if let Some(choice) = host.select("Pick", &["a", "b"]).await? {
                host.notify_info(format!("You picked {choice}")).await?;
            }
            Ok(())
        })
        .command(
            "hello-panel",
            "Open the hello panel",
            |_args, ctx: CommandCtx| hello_panel(ctx.host().clone()),
        )
        .on("before_agent_start", |_ev: EventCtx| async move {
            Ok(EventReply::section("hello", "Greet people by name."))
        })
        .on_start(|host: Host| async move {
            if host.has_ui() {
                let line = style::fg(Color::Green, "hello ready");
                host.set_widget("hello", [line], Placement::BelowEditor)
                    .await?;
            }
            Ok(())
        })
        .on_surface(|event: SurfaceEvent, host: Host| async move {
            match event {
                SurfaceEvent::Resize { width, .. } if event.widget_key() == Some("hello") => {
                    host.set_widget("hello", widget_lines(width), Placement::BelowEditor)
                        .await
                }
                _ => Ok(()),
            }
        })
        .run()
}
