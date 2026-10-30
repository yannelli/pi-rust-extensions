mod common;

use common::{TestHost, ctx};
use pi_ext_sdk::prelude::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct GreetArgs {
    name: String,
}

fn tools() -> Extension {
    Extension::new("tools")
        .tool(
            ToolSpec::new("greet", "Greet"),
            |args: GreetArgs, ctx: ToolCtx| async move {
                ctx.update("working")?;
                Ok(ToolResult::text(format!("Hello, {}", args.name))
                    .details(json!({ "id": ctx.tool_call_id() })))
            },
        )
        .tool(
            ToolSpec::new("wait", "Wait for cancel"),
            |_: Value, ctx: ToolCtx| async move {
                ctx.cancelled().await;
                Err::<ToolResult, _>(Error::msg("stopped"))
            },
        )
        .tool(
            ToolSpec::new("partial", "Answer on cancel"),
            |_: Value, ctx: ToolCtx| async move {
                ctx.cancelled().await;
                Ok("partial result")
            },
        )
        .tool(
            ToolSpec::new("stuck", "Ignore cancel"),
            |_: Value, _: ToolCtx| async move {
                std::future::pending::<()>().await;
                Ok("never")
            },
        )
        .tool(
            ToolSpec::new("boom", "Panic"),
            |_: Value, _: ToolCtx| async move {
                if true {
                    panic!("kaboom");
                }
                Ok("unreachable")
            },
        )
}

fn execute(id: &str, name: &str, params: Value) -> Value {
    json!({ "toolCallId": id, "name": name, "params": params, "ctx": ctx() })
}

#[tokio::test]
async fn tool_sends_update_then_result() {
    let mut host = TestHost::start(tools());
    host.initialize().await;
    host.request(
        2,
        "tool.execute",
        execute("c1", "greet", json!({ "name": "Ada" })),
    )
    .await;
    let update = host.expect_notification("tool.update").await;
    assert_eq!(
        update,
        json!({ "toolCallId": "c1", "partial": { "content": [{ "type": "text", "text": "working" }] } })
    );
    let response = host.expect_response(2).await;
    assert_eq!(
        response["result"],
        json!({ "content": [{ "type": "text", "text": "Hello, Ada" }], "details": { "id": "c1" } })
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn invalid_args_and_unknown_tool_are_invalid_params() {
    let mut host = TestHost::start(tools());
    host.initialize().await;
    host.request(2, "tool.execute", execute("c1", "greet", json!({})))
        .await;
    assert_eq!(host.expect_response(2).await["error"]["code"], -32602);
    host.request(3, "tool.execute", execute("c2", "missing", json!({})))
        .await;
    assert_eq!(host.expect_response(3).await["error"]["code"], -32602);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn cancelled_tool_error_answers_cancelled() {
    let mut host = TestHost::start(tools());
    host.initialize().await;
    host.request(4, "tool.execute", execute("c4", "wait", json!({})))
        .await;
    host.notify("$/cancelRequest", json!({ "id": 4 })).await;
    assert_eq!(host.expect_response(4).await["error"]["code"], -32800);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn cancelled_tool_may_still_return_a_result() {
    let mut host = TestHost::start(tools());
    host.initialize().await;
    host.request(4, "tool.execute", execute("c4", "partial", json!({})))
        .await;
    host.notify("$/cancelRequest", json!({ "id": 4 })).await;
    let response = host.expect_response(4).await;
    assert_eq!(response["result"]["content"][0]["text"], "partial result");
    host.finish().await.unwrap();
}

#[tokio::test]
async fn stuck_tool_is_dropped_after_grace() {
    let mut host = TestHost::start(tools());
    host.initialize().await;
    host.request(5, "tool.execute", execute("c5", "stuck", json!({})))
        .await;
    host.notify("$/cancelRequest", json!({ "id": 5 })).await;
    assert_eq!(host.expect_response(5).await["error"]["code"], -32800);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn panicking_tool_answers_internal_error() {
    let mut host = TestHost::start(tools());
    host.initialize().await;
    host.request(6, "tool.execute", execute("c6", "boom", json!({})))
        .await;
    let response = host.expect_response(6).await;
    assert_eq!(response["error"]["code"], -32603);
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("kaboom")
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn tools_run_concurrently() {
    let mut host = TestHost::start(tools());
    host.initialize().await;
    host.request(7, "tool.execute", execute("c7", "wait", json!({})))
        .await;
    host.request(
        8,
        "tool.execute",
        execute("c8", "greet", json!({ "name": "Bo" })),
    )
    .await;
    host.expect_notification("tool.update").await;
    assert_eq!(
        host.expect_response(8).await["result"]["content"][0]["text"],
        "Hello, Bo"
    );
    host.notify("$/cancelRequest", json!({ "id": 7 })).await;
    assert_eq!(host.expect_response(7).await["error"]["code"], -32800);
    host.finish().await.unwrap();
}
