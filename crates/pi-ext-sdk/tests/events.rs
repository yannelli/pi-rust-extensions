mod common;

use common::{TestHost, ctx};
use pi_ext_sdk::event_types::ToolCallEvent;
use pi_ext_sdk::prelude::*;

#[tokio::test]
async fn event_handlers_patch_and_result() {
    let ext = Extension::new("guard")
        .on("tool_call", |ev: EventCtx| async move {
            let call: ToolCallEvent = ev.event_as()?;
            assert_eq!(ev.snapshot().cwd, "/work");
            if call.tool_name == "bash" {
                return Ok(EventReply::patch(
                    json!({ "input": { "command": "ls -la", "timeout": null } }),
                ));
            }
            Ok(EventReply::none())
        })
        .on("tool_call", |ev: EventCtx| async move {
            let command = ev.event()["input"]["command"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            assert!(ev.event()["input"].get("timeout").is_none());
            Ok(EventReply::block(format!("blocked {command}")))
        });
    let mut host = TestHost::start(ext);
    let init = host.initialize().await;
    assert_eq!(init["events"], json!(["tool_call"]));
    let event = json!({
        "type": "tool_call", "toolCallId": "t1", "toolName": "bash",
        "input": { "command": "ls", "timeout": 5 }
    });
    host.request(
        3,
        "event",
        json!({ "name": "tool_call", "event": event, "ctx": ctx() }),
    )
    .await;
    let response = host.expect_response(3).await;
    assert_eq!(
        response["result"],
        json!({
            "result": { "block": true, "reason": "blocked ls -la" },
            "patch": { "input": { "command": "ls -la", "timeout": null } }
        })
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn single_handler_section_patch() {
    let ext = Extension::new("hello").on("before_agent_start", |_| async move {
        Ok(EventReply::section("hello", "Greet people by name."))
    });
    let mut host = TestHost::start(ext);
    host.initialize().await;
    let event = json!({ "type": "before_agent_start", "prompt": "hi", "systemPromptOptions": { "sections": {} } });
    host.request(
        3,
        "event",
        json!({ "name": "before_agent_start", "event": event, "ctx": ctx() }),
    )
    .await;
    let response = host.expect_response(3).await;
    assert_eq!(
        response["result"],
        json!({ "patch": { "systemPromptOptions": { "sections": { "hello": "Greet people by name." } } } })
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn failing_handler_answers_error() {
    let ext = Extension::new("fail").on("input", |_| async move {
        Err::<EventReply, _>(Error::msg("bad input"))
    });
    let mut host = TestHost::start(ext);
    host.initialize().await;
    let event = json!({ "type": "input", "text": "x", "source": "interactive" });
    host.request(
        3,
        "event",
        json!({ "name": "input", "event": event, "ctx": ctx() }),
    )
    .await;
    let response = host.expect_response(3).await;
    assert_eq!(response["error"]["message"], "bad input");
    host.finish().await.unwrap();
}

#[tokio::test]
async fn events_subscribe_delivers_messages() {
    let ext = Extension::new("bus").command("listen", "", |_, ctx: CommandCtx| async move {
        let host = ctx.host().clone();
        let mut sub = host.subscribe("demo:ping").await?;
        let data = sub.recv().await.unwrap_or_default();
        host.notify_info(format!("got {}", data["n"])).await?;
        drop(sub);
        host.emit("demo:pong", json!({ "ok": true })).await
    });
    let mut host = TestHost::start(ext);
    host.initialize().await;
    host.request(
        4,
        "command.run",
        json!({ "name": "listen", "args": "", "ctx": ctx() }),
    )
    .await;
    let (id, params) = host.expect_request("events.subscribe").await;
    assert_eq!(params, json!({ "channel": "demo:ping" }));
    host.answer(&id, Value::Null).await;
    host.notify(
        "events.message",
        json!({ "channel": "other", "data": { "n": 0 } }),
    )
    .await;
    host.notify(
        "events.message",
        json!({ "channel": "demo:ping", "data": { "n": 7 } }),
    )
    .await;
    let (id, params) = host.expect_request("ui.notify").await;
    assert_eq!(params["message"], "got 7");
    host.answer(&id, Value::Null).await;
    let params = host.expect_notification("events.unsubscribe").await;
    assert_eq!(params, json!({ "channel": "demo:ping" }));
    let (id, params) = host.expect_request("events.emit").await;
    assert_eq!(
        params,
        json!({ "channel": "demo:pong", "data": { "ok": true } })
    );
    host.answer(&id, Value::Null).await;
    assert_eq!(host.expect_response(4).await["result"], Value::Null);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn entries_of_filters_branch() {
    let ext = Extension::new("todo").command("count", "", |_, ctx: CommandCtx| async move {
        let host = ctx.host();
        host.append_entry("todo", &json!({ "n": 3 })).await?;
        let items: Vec<Value> = host.entries_of("todo").await?;
        host.notify_info(format!("{} entries", items.len())).await
    });
    let mut host = TestHost::start(ext);
    host.initialize().await;
    host.request(
        4,
        "command.run",
        json!({ "name": "count", "args": "", "ctx": ctx() }),
    )
    .await;
    let (id, params) = host.expect_request("session.appendEntry").await;
    assert_eq!(params, json!({ "customType": "todo", "data": { "n": 3 } }));
    host.answer(&id, Value::Null).await;
    let (id, _) = host.expect_request("session.getBranch").await;
    let branch = json!([
        { "type": "custom", "customType": "todo", "data": { "n": 1 } },
        { "type": "message", "message": {} },
        { "type": "custom", "customType": "todo", "data": { "n": 3 } }
    ]);
    host.answer(&id, branch).await;
    let (id, params) = host.expect_request("ui.notify").await;
    assert_eq!(params["message"], "2 entries");
    host.answer(&id, Value::Null).await;
    assert_eq!(host.expect_response(4).await["result"], Value::Null);
    host.finish().await.unwrap();
}
