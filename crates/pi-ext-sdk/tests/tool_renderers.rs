//! Renderers of tools the extension does not add (`toolRenderers`), `refreshMs`, the theme, and
//! the working indicator.

mod common;

use common::{TestHost, ctx};
use pi_ext_sdk::prelude::*;
use pi_ext_sdk::{SPINNER, ToolCallRender, ToolResultRender};

fn compact() -> Extension {
    Extension::new("compact")
        .tool(ToolSpec::new("own", "Own"), |_: Value, _: ToolCtx| async {
            Ok("ok")
        })
        .render_tool_call("own", |_: ToolCallRender, _| async {
            Ok(Some(vec!["own".to_string()]))
        })
        .render_tool_call("bash", |request: ToolCallRender, _| async move {
            let line = format!(
                "{SPINNER} $ {}",
                request.args["command"].as_str().unwrap_or("")
            );
            Ok(Rendered::new(vec![line]).refresh_after(1000))
        })
        .render_tool_result("bash", |request: ToolResultRender, _| async move {
            Ok(Some(vec![request.text()]))
        })
        .render_tool_result("read", |_: ToolResultRender, _| async { Ok(None) })
        .render_shell("bash", "self")
        .on_theme_change(|theme: Theme, host: Host| async move {
            host.notify_info(format!("theme {}", theme.name.unwrap_or_default()))
                .await
        })
        .command("look", "", |_, ctx: CommandCtx| async move {
            let host = ctx.host();
            let theme = host.theme().await?;
            host.set_working_indicator(Some(&["a", "b"]), Some(90))
                .await?;
            host.set_working_indicator::<&str>(None, None).await?;
            host.set_working_message(Some(&theme.fg("accent", "busy")))
                .await?;
            host.set_working_message(None).await?;
            host.set_hidden_thinking_label(Some("Thought")).await
        })
}

#[tokio::test]
async fn initialize_lists_renderers_of_tools_the_extension_does_not_add() {
    let mut host = TestHost::start(compact());
    let init = host.initialize().await;
    assert_eq!(
        init["toolRenderers"],
        json!([
            { "toolName": "bash", "renderCall": true, "renderResult": true, "renderShell": "self" },
            { "toolName": "read", "renderCall": false, "renderResult": true }
        ])
    );
    assert_eq!(init["tools"][0]["renderCall"], json!(true));
    assert!(
        init["events"]
            .as_array()
            .unwrap()
            .contains(&json!("theme_changed")),
        "{init}"
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn renderers_answer_refresh_times_and_spinner_tokens() {
    let mut host = TestHost::start(compact());
    host.initialize().await;
    let call =
        json!({ "toolName": "bash", "args": { "command": "ls" }, "context": {}, "width": 40 });
    host.request(2, "render.toolCall", call).await;
    let answer = host.expect_response(2).await["result"].clone();
    assert_eq!(
        answer,
        json!({ "lines": [format!("{SPINNER} $ ls")], "refreshMs": 1000 })
    );
    let result =
        json!({ "toolName": "read", "result": { "content": [] }, "context": {}, "width": 40 });
    host.request(3, "render.toolResult", result).await;
    assert_eq!(host.expect_response(3).await["result"], Value::Null);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn theme_changes_and_working_indicator_calls_cross_the_protocol() {
    let mut host = TestHost::start(compact());
    host.initialize().await;
    host.notify(
        "theme.changed",
        json!({ "theme": { "name": "light", "appearance": "light" } }),
    )
    .await;
    let (id, params) = host.expect_request("ui.notify").await;
    assert_eq!(params["message"], "theme light");
    host.answer(&id, Value::Null).await;
    let theme =
        json!({ "name": "dark", "fg": { "accent": "\u{1b}[38;2;1;2;3m" }, "bg": {}, "dim": [] });
    let mut seen = Vec::new();
    let reply = host
        .drive(
            4,
            "command.run",
            json!({ "name": "look", "args": "", "ctx": ctx() }),
            |method, params| {
                seen.push((method.to_string(), params.clone()));
                if method == "ui.getTheme" {
                    theme.clone()
                } else {
                    Value::Null
                }
            },
        )
        .await;
    assert_eq!(reply["result"], Value::Null, "{reply}");
    let expected = [
        ("ui.getTheme", json!({})),
        (
            "ui.setWorkingIndicator",
            json!({ "frames": ["a", "b"], "intervalMs": 90 }),
        ),
        ("ui.setWorkingIndicator", json!({})),
        (
            "ui.setWorkingMessage",
            json!({ "message": "\u{1b}[38;2;1;2;3mbusy\u{1b}[39m" }),
        ),
        ("ui.setWorkingMessage", json!({})),
        ("ui.setHiddenThinkingLabel", json!({ "label": "Thought" })),
    ];
    let expected: Vec<(String, Value)> = expected
        .into_iter()
        .map(|(m, p)| (m.to_string(), p))
        .collect();
    assert_eq!(seen, expected);
    host.finish().await.unwrap();
}
