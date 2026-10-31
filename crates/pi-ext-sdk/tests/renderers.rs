//! `tool.prepare`, the `render.*` requests, `settings.set`, `keybindings.get`, `auth.login`, and
//! the `oauth: { builtin }` provider config.

mod common;

use common::{TestHost, ctx};
use pi_ext_sdk::prelude::*;
use pi_ext_sdk::{
    EntryRender, MessageRender, OAuthSpec, SettingsScope, ToolCallRender, ToolResultRender,
};

fn draw() -> ToolSpec {
    ToolSpec::new("draw", "Draw").parameters(json!({
        "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"]
    }))
}

fn renderers() -> Extension {
    Extension::new("renderers")
        .render_tool_call("draw", |request: ToolCallRender, _| async move {
            Ok(Some(vec![format!("call {} at {}", request.args["text"], request.width)]))
        })
        .tool(draw(), |args: Value, _: ToolCtx| async move { Ok(format!("drew {}", args["text"])) })
        .render_tool_result("draw", |request: ToolResultRender, _| async move {
            Ok(Some(vec![format!("{} partial={}", request.text(), request.options.is_partial)]))
        })
        .prepare_arguments("draw", |mut args: Value, _| async move {
            if let Some(number) = args["text"].as_i64() {
                args["text"] = json!(number.to_string());
            }
            Ok(args)
        })
        .message_renderer("card", |request: MessageRender, _| async move {
            Ok(Some(vec![format!("card {}", request.message["content"])]))
        })
        .entry_renderer("note", |request: EntryRender, _| async move {
            Ok(request.data::<Value>().map(|data| vec![format!("note {data} open={}", request.options.expanded)]))
        })
        .provider(
            "anthropic-2",
            json!({ "name": "Anthropic #2", "oauth": OAuthSpec::builtin("anthropic").name("Anthropic #2").to_value() }),
        )
        .command("store", "", |_, ctx: CommandCtx| async move {
            let host = ctx.host();
            host.set_settings("renderers", json!({ "on": true }), SettingsScope::Project).await?;
            let keys = host.keybindings(&["app.tools.expand"]).await?;
            let logged_in = host.login("anthropic-2").await?;
            host.notify_info(format!("{keys:?} {logged_in}")).await
        })
}

#[tokio::test]
async fn initialize_lists_the_flags_of_renderers_and_prepare() {
    let mut host = TestHost::start(renderers());
    let init = host.initialize().await;
    let tool = &init["tools"][0];
    assert_eq!(
        (
            &tool["prepareArguments"],
            &tool["renderCall"],
            &tool["renderResult"]
        ),
        (&json!(true), &json!(true), &json!(true))
    );
    assert_eq!(init["messageRenderers"], json!(["card"]));
    assert_eq!(init["entryRenderers"], json!(["note"]));
    let config = &init["providers"][0]["config"];
    assert_eq!(
        config["oauth"],
        json!({ "builtin": "anthropic", "name": "Anthropic #2" })
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn plain_tools_send_no_new_flags() {
    let ext = Extension::new("plain").tool(ToolSpec::new("t", "T"), |_: Value, _: ToolCtx| async {
        Ok("ok")
    });
    let mut host = TestHost::start(ext);
    let init = host.initialize().await;
    let mut keys: Vec<&String> = init["tools"][0].as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["description", "name", "parameters"]);
    assert!(init.get("messageRenderers").is_none() && init.get("entryRenderers").is_none());
    host.finish().await.unwrap();
}

#[tokio::test]
async fn prepare_and_render_requests_answer_lines() {
    let mut host = TestHost::start(renderers());
    host.initialize().await;
    host.request(
        2,
        "tool.prepare",
        json!({ "name": "draw", "args": { "text": 5 } }),
    )
    .await;
    assert_eq!(
        host.expect_response(2).await["result"],
        json!({ "text": "5" })
    );
    let call = json!({ "toolName": "draw", "toolCallId": "c1", "args": { "text": "hi" }, "context": {}, "width": 40 });
    host.request(3, "render.toolCall", call).await;
    assert_eq!(
        host.expect_response(3).await["result"],
        json!({ "lines": ["call \"hi\" at 40"] })
    );
    let result = json!({ "toolName": "draw", "result": { "content": [{ "type": "text", "text": "drew hi" }] },
        "options": { "expanded": false, "isPartial": true }, "context": {}, "width": 30 });
    host.request(4, "render.toolResult", result).await;
    assert_eq!(
        host.expect_response(4).await["result"],
        json!({ "lines": ["drew hi partial=true"] })
    );
    let message = json!({ "customType": "card", "message": { "content": "teal" }, "options": {}, "width": 20 });
    host.request(5, "render.message", message).await;
    assert_eq!(
        host.expect_response(5).await["result"],
        json!({ "lines": ["card \"teal\""] })
    );
    let entry = json!({ "customType": "note", "entry": { "data": 3 }, "options": { "expanded": true }, "width": 20 });
    host.request(6, "render.entry", entry).await;
    assert_eq!(
        host.expect_response(6).await["result"],
        json!({ "lines": ["note 3 open=true"] })
    );
    let unknown = json!({ "customType": "other", "message": {}, "width": 20 });
    host.request(7, "render.message", unknown).await;
    assert_eq!(host.expect_response(7).await["result"], Value::Null);
    host.request(
        8,
        "tool.prepare",
        json!({ "name": "other", "args": { "a": 1 } }),
    )
    .await;
    assert_eq!(host.expect_response(8).await["result"], json!({ "a": 1 }));
    host.finish().await.unwrap();
}

#[tokio::test]
async fn settings_keybindings_and_login_requests() {
    let mut host = TestHost::start(renderers());
    host.initialize().await;
    host.request(
        2,
        "command.run",
        json!({ "name": "store", "args": "", "ctx": ctx() }),
    )
    .await;
    let (id, params) = host.expect_request("settings.set").await;
    assert_eq!(
        params,
        json!({ "namespace": "renderers", "value": { "on": true }, "scope": "project" })
    );
    host.answer(&id, Value::Null).await;
    let (id, params) = host.expect_request("keybindings.get").await;
    assert_eq!(params, json!({ "ids": ["app.tools.expand"] }));
    host.answer(&id, json!({ "app.tools.expand": "ctrl+o" }))
        .await;
    let (id, params) = host.expect_request("auth.login").await;
    assert_eq!(params, json!({ "provider": "anthropic-2" }));
    host.answer(&id, json!(true)).await;
    let (id, params) = host.expect_request("ui.notify").await;
    assert_eq!(
        params["message"],
        json!("{\"app.tools.expand\": \"ctrl+o\"} true")
    );
    host.answer(&id, Value::Null).await;
    assert_eq!(host.expect_response(2).await["result"], Value::Null);
    host.finish().await.unwrap();
}
