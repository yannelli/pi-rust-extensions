#[path = "../../tests/common/mod.rs"]
mod common;

use std::sync::Arc;

use pi_ext_sdk::{Extension, ToolCtx, ToolSpec};
use serde_json::json;

use crate::browser::BrowserArgs;
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
