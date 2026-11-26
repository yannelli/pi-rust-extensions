#[path = "../../tests/common/mod.rs"]
mod common;

use std::sync::Arc;

use pi_ext_sdk::{Extension, ToolCtx, ToolSpec};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::browser::{BrowserArgs, Managed};
use crate::live::Live;
use crate::runtime::Runtime;

#[tokio::test]
async fn evaluation_rejects_a_replaced_state_before_connecting() {
    let runtime = Arc::new(Runtime::default());
    let mut old = super::logic::observation("old");
    old.target.page_id = Some("P1".into());
    old.target.cdp_port = Some(1);
    runtime.store().put_state(old.clone());
    old.capture.state_id = "new".into();
    runtime.store().put_state(old);
    let extension = Extension::new("state-check").tool(
        ToolSpec::new("evaluate_browser", "Evaluate a browser expression"),
        move |args: BrowserArgs, ctx: ToolCtx| {
            let runtime = runtime.clone();
            async move {
                Live {
                    host: ctx.host(),
                    rt: &runtime,
                    cancel: ctx.cancel_token(),
                }
                .evaluate_browser(args)
                .await
            }
        },
    );
    let mut host = common::TestHost::start(extension);
    host.initialize().await;
    host.request(
        1,
        "tool.execute",
        json!({
            "toolCallId":"stale", "name":"evaluate_browser",
            "params":{"stateId":"old", "expression":"1 + 1"}, "ctx":common::ctx()
        }),
    )
    .await;
    let response = host.expect_response(1).await;
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Stale state 'old'")
    );
    host.finish().await.unwrap();
}

/// Answers `/json/version` with `version` and `/json/list` with no pages.
async fn mock_devtools(version: &'static str) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut request = [0; 1024];
            let read = stream.read(&mut request).await.unwrap();
            let body = if request[..read].starts_with(b"GET /json/list") {
                "[]"
            } else {
                version
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    port
}

#[tokio::test]
async fn launch_with_a_port_keeps_the_managed_browser() {
    let port =
        mock_devtools(r#"{"webSocketDebuggerUrl":"ws://127.0.0.1/devtools/browser/b"}"#).await;
    let other = mock_devtools("{}").await;
    let runtime = Arc::new(Runtime::default());
    let child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    *runtime.browser() = Some(Managed {
        port,
        name: "Chrome",
        pid: i64::from(child.id()),
        child: Some(child),
    });
    let rt = runtime.clone();
    let extension = Extension::new("launch-check").tool(
        ToolSpec::new("launch_browser", "Launch a browser"),
        move |args: BrowserArgs, ctx: ToolCtx| {
            let runtime = rt.clone();
            async move {
                Live {
                    host: ctx.host(),
                    rt: &runtime,
                    cancel: ctx.cancel_token(),
                }
                .launch_browser(args)
                .await
            }
        },
    );
    let mut host = common::TestHost::start(extension);
    host.initialize().await;
    for (id, port, error) in [
        (1, other, "not a DevTools endpoint"),
        (2, port, "no page target"),
    ] {
        host.request(
            id,
            "tool.execute",
            json!({
                "toolCallId":"launch", "name":"launch_browser",
                "params":{"port":port}, "ctx":common::ctx()
            }),
        )
        .await;
        let response = host.expect_response(id).await;
        let message = response["error"]["message"].as_str().unwrap();
        assert!(message.contains(error), "{message}");
    }
    host.finish().await.unwrap();
    let mut browser = runtime.browser();
    let child = browser.as_mut().unwrap().child.as_mut().unwrap();
    assert!(child.try_wait().unwrap().is_none());
}
