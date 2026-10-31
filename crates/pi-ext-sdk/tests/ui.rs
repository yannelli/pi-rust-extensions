mod common;

use common::{TestHost, ctx};
use pi_ext_sdk::prelude::*;

#[tokio::test]
async fn command_runs_select_round_trip() {
    let ext =
        Extension::new("hello").command("hello", "Say hello", |args, ctx: CommandCtx| async move {
            let host = ctx.host();
            let choice = host.select("Pick", &["a", "b"]).await?;
            host.notify_info(format!("{args}: {}", choice.unwrap_or_default()))
                .await
        });
    let mut host = TestHost::start(ext);
    host.initialize().await;
    host.request(
        4,
        "command.run",
        json!({ "name": "hello", "args": "x", "ctx": ctx() }),
    )
    .await;
    let (id, params) = host.expect_request("ui.select").await;
    assert_eq!(params, json!({ "title": "Pick", "options": ["a", "b"] }));
    host.answer(&id, json!("b")).await;
    let (id, params) = host.expect_request("ui.notify").await;
    assert_eq!(params["message"], "x: b");
    host.answer(&id, Value::Null).await;
    assert_eq!(host.expect_response(4).await["result"], Value::Null);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn dropped_dialog_sends_cancel_request() {
    let ext = Extension::new("timeout").command("ask", "", |_, ctx: CommandCtx| async move {
        let ask = ctx.host().confirm("Sure?", "Really");
        let answer = tokio::time::timeout(std::time::Duration::from_millis(50), ask).await;
        assert!(answer.is_err());
        Ok(())
    });
    let mut host = TestHost::start(ext);
    host.initialize().await;
    host.request(
        4,
        "command.run",
        json!({ "name": "ask", "args": "", "ctx": ctx() }),
    )
    .await;
    let (id, _) = host.expect_request("ui.confirm").await;
    let params = host.expect_notification("$/cancelRequest").await;
    assert_eq!(params["id"], id);
    assert_eq!(host.expect_response(4).await["result"], Value::Null);
    host.finish().await.unwrap();
}

fn panel_extension() -> Extension {
    Extension::new("panel").command("panel", "", |_, ctx: CommandCtx| async move {
        let mut panel = ctx.host().panel(["Panel", "q closes"]).open().await?;
        loop {
            match panel.next_event().await {
                PanelEvent::Resize(width) => panel.set_lines([format!("width {width}")])?,
                PanelEvent::Key(key) if key.is_cancel() => break,
                PanelEvent::Key(key) => panel.set_lines([format!("key {key:?}")])?,
                PanelEvent::Closed => return Ok(()),
            }
        }
        let result = panel.close("closed").await?;
        ctx.host().notify_info(format!("result {result}")).await
    })
}

#[tokio::test]
async fn panel_open_key_resize_close() {
    let mut host = TestHost::start(panel_extension());
    host.initialize().await;
    host.request(
        4,
        "command.run",
        json!({ "name": "panel", "args": "", "ctx": ctx() }),
    )
    .await;
    let (custom_id, params) = host.expect_request("ui.custom").await;
    let surface = params["surface"].clone();
    assert_eq!(params["lines"], json!(["Panel", "q closes"]));
    host.notify("ui.resize", json!({ "surface": surface, "width": 80 }))
        .await;
    let params = host.expect_notification("ui.update").await;
    assert_eq!(params, json!({ "surface": surface, "lines": ["width 80"] }));
    host.notify("ui.key", json!({ "surface": surface, "data": "\u{1b}[A" }))
        .await;
    let params = host.expect_notification("ui.update").await;
    assert_eq!(params["lines"], json!(["key Up"]));
    host.notify("ui.key", json!({ "surface": surface, "data": "q" }))
        .await;
    let params = host.expect_notification("ui.close").await;
    assert_eq!(params, json!({ "surface": surface, "result": "closed" }));
    host.answer(&custom_id, json!({ "result": "closed" })).await;
    let (id, params) = host.expect_request("ui.notify").await;
    assert_eq!(params["message"], "result \"closed\"");
    host.answer(&id, Value::Null).await;
    assert_eq!(host.expect_response(4).await["result"], Value::Null);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn panel_height_tracks_resize_and_old_hosts_leave_it_unset() {
    let ext = Extension::new("size").command("size", "", |_, ctx: CommandCtx| async move {
        let mut panel = ctx.host().panel(["size"]).open().await?;
        while let PanelEvent::Resize(width) = panel.next_event().await {
            panel.set_lines([format!("{width}x{:?}", panel.height())])?;
        }
        Ok(())
    });
    let mut host = TestHost::start(ext);
    host.initialize().await;
    host.request(
        4,
        "command.run",
        json!({ "name": "size", "args": "", "ctx": ctx() }),
    )
    .await;
    let (custom_id, params) = host.expect_request("ui.custom").await;
    let surface = params["surface"].clone();
    let resizes = [
        (json!({ "surface": surface, "width": 80 }), "80xNone"),
        (
            json!({ "surface": surface, "width": 80, "height": 12 }),
            "80xSome(12)",
        ),
        (json!({ "surface": surface, "height": 16 }), "80xSome(16)"),
        (
            json!({ "surface": surface, "width": 40, "height": 0 }),
            "40xSome(16)",
        ),
    ];
    for (params, line) in resizes {
        host.notify("ui.resize", params).await;
        let update = host.expect_notification("ui.update").await;
        assert_eq!(update["lines"], json!([line]));
    }
    host.notify("ui.closed", json!({ "surface": surface }))
        .await;
    host.answer(&custom_id, json!({ "result": null })).await;
    assert_eq!(host.expect_response(4).await["result"], Value::Null);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn panel_open_is_sent_before_first_update() {
    let ext = Extension::new("order").command("p", "", |_, ctx: CommandCtx| async move {
        let panel = ctx.host().panel(["loading"]).open().await?;
        panel.set_lines(["ready"])?;
        panel.result().await.map(drop)
    });
    let mut host = TestHost::start(ext);
    host.initialize().await;
    for id in 0..20 {
        host.request(
            10 + id,
            "command.run",
            json!({ "name": "p", "args": "", "ctx": ctx() }),
        )
        .await;
        let (custom_id, params) = host.expect_request("ui.custom").await;
        assert_eq!(params["lines"], json!(["loading"]));
        let update = host.expect_notification("ui.update").await;
        assert_eq!(update["lines"], json!(["ready"]));
        host.answer(&custom_id, json!({ "result": null })).await;
        assert_eq!(host.expect_response(10 + id).await["result"], Value::Null);
    }
    host.finish().await.unwrap();
}

#[tokio::test]
async fn panel_closed_by_host() {
    let mut host = TestHost::start(panel_extension());
    host.initialize().await;
    host.request(
        4,
        "command.run",
        json!({ "name": "panel", "args": "", "ctx": ctx() }),
    )
    .await;
    let (custom_id, params) = host.expect_request("ui.custom").await;
    host.notify("ui.closed", json!({ "surface": params["surface"] }))
        .await;
    host.answer(&custom_id, json!({ "result": null })).await;
    assert_eq!(host.expect_response(4).await["result"], Value::Null);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn panel_without_ui_fails() {
    let mut host = TestHost::start(panel_extension());
    host.initialize().await;
    let ctx = json!({ "cwd": "/work", "hasUI": false });
    host.request(
        4,
        "command.run",
        json!({ "name": "panel", "args": "", "ctx": ctx }),
    )
    .await;
    assert_eq!(host.expect_response(4).await["error"]["code"], -32001);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn widget_resize_reaches_surface_handler() {
    let ext = Extension::new("widget").on_surface(|event: SurfaceEvent, host: Host| async move {
        let SurfaceEvent::Resize { width, .. } = event else {
            return Ok(());
        };
        let key = event.widget_key().unwrap_or("none").to_owned();
        let line = style::truncate("a long line of widget text", width);
        host.set_widget(&key, [line], Placement::BelowEditor).await
    });
    let mut host = TestHost::start(ext);
    host.initialize().await;
    host.notify("ui.resize", json!({ "surface": "widget:todo", "width": 8 }))
        .await;
    let (id, params) = host.expect_request("ui.setWidget").await;
    assert_eq!(
        params,
        json!({ "key": "todo", "lines": ["a long \u{2026}"], "placement": "belowEditor" })
    );
    host.answer(&id, Value::Null).await;
    host.finish().await.unwrap();
}
