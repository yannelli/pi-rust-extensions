//! Custom rows: a tool that accepts a number or a string, draws its own call and result rows, and
//! posts a card message and a session note that it also draws. `/cards-theme` stores a setting.

use std::process::ExitCode;

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::style::{self, Color};
use pi_ext_sdk::{
    CustomMessage, EntryRender, MessageRender, SettingsScope, ToolCallRender, ToolResultRender,
};

fn color_of(name: &str) -> Color {
    if name == "teal" {
        Color::Cyan
    } else {
        Color::Magenta
    }
}

pub fn main() -> ExitCode {
    let card = ToolSpec::new("card", "Show a colored card").parameters(json!({
        "type": "object", "properties": { "color": { "type": "string" } }, "required": ["color"]
    }));
    Extension::new("cards")
        .tool(card, |args: Value, ctx: ToolCtx| async move {
            let color = args["color"].as_str().unwrap_or("plum").to_owned();
            let host = ctx.host();
            host.send_message(CustomMessage::new("card", color.clone()).display(true))
                .await?;
            host.append_entry("card-note", &json!({ "color": color }))
                .await?;
            Ok(format!("posted a {color} card"))
        })
        .prepare_arguments("card", |mut args: Value, _| async move {
            if let Some(index) = args["color"].as_u64() {
                args["color"] = json!(if index == 0 { "teal" } else { "plum" });
            }
            Ok(args)
        })
        .render_tool_call("card", |req: ToolCallRender, _| async move {
            let color = req.args["color"].as_str().unwrap_or("?");
            Ok(Some(vec![style::truncate(
                &format!("{} {color}", style::bold("card")),
                req.width,
            )]))
        })
        .render_tool_result("card", |req: ToolResultRender, host: Host| async move {
            let keys = host
                .keybindings(&["app.tools.expand"])
                .await
                .unwrap_or_default();
            let hint = keys
                .get("app.tools.expand")
                .map_or(String::new(), |key| format!(" ({key} to expand)"));
            Ok(Some(vec![style::truncate(
                &format!("{}{}", req.text(), style::dim(&hint)),
                req.width,
            )]))
        })
        .message_renderer("card", |req: MessageRender, _| async move {
            let color = req.message["content"].as_str().unwrap_or("plum").to_owned();
            let bar = "█".repeat(req.width.min(24));
            Ok(Some(vec![
                style::fg(color_of(&color), &bar),
                style::truncate(&color, req.width),
            ]))
        })
        .entry_renderer("card-note", |req: EntryRender, _| async move {
            let color = req
                .data::<Value>()
                .and_then(|data| data["color"].as_str().map(str::to_owned));
            Ok(color.map(|color| {
                vec![style::dim(&style::truncate(
                    &format!("note: {color} card"),
                    req.width,
                ))]
            }))
        })
        .command(
            "cards-theme",
            "Store the default card color",
            |args, ctx: CommandCtx| async move {
                let color = if args.trim().is_empty() {
                    "teal"
                } else {
                    args.trim()
                };
                ctx.host()
                    .set_settings("cards", json!({ "color": color }), SettingsScope::Global)
                    .await
            },
        )
        .run()
}
