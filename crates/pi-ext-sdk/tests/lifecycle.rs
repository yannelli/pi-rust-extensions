mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use common::TestHost;
use pi_ext_sdk::prelude::*;
use pi_ext_sdk::{CompletionItem, Suggestions};

fn full_extension() -> Extension {
    Extension::new("full")
        .tool(
            ToolSpec::new("greet", "Greet someone")
                .label("Greet")
                .parameters(
                    json!({ "type": "object", "properties": { "name": { "type": "string" } } }),
                )
                .guideline("Use it for greetings"),
            |_: Value, _: ToolCtx| async move { Ok("hi") },
        )
        .command("hello", "Say hello", |_, _| async move { Ok(()) })
        .completions("hello", |prefix: String, _| async move {
            Ok(Some(vec![CompletionItem::new(format!("{prefix}world"))]))
        })
        .shortcut("ctrl+shift+h", "Hello", |_| async move { Ok(()) })
        .flag(
            FlagSpec::boolean("plan")
                .description("Plan mode")
                .default(false),
        )
        .on("session_start", |_| async move { Ok(()) })
        .on("tool_call", |_| async move { Ok(()) })
        .on("session_start", |_| async move { Ok(()) })
        .provider(
            "local",
            json!({ "baseUrl": "http://localhost:1234", "api": "openai-completions" }),
        )
        .virtual_model(
            json!({ "provider": "router", "id": "auto" }),
            |req, _| async move { Ok(json!({ "model": format!("{}/{}", req.provider, req.id) })) },
        )
        .autocomplete(&["@"], |req, _| async move {
            let prefix = req
                .before_cursor()
                .rsplit(' ')
                .next()
                .unwrap_or("")
                .to_owned();
            Ok(Some(Suggestions {
                items: vec![CompletionItem::new("@file")],
                prefix,
            }))
        })
}

#[tokio::test]
async fn initialize_returns_registrations() {
    let mut host = TestHost::start(full_extension());
    let result = host.initialize().await;
    assert_eq!(result["protocolVersion"], 1);
    assert_eq!(result["tools"][0]["name"], "greet");
    assert_eq!(result["tools"][0]["label"], "Greet");
    assert_eq!(
        result["tools"][0]["promptGuidelines"],
        json!(["Use it for greetings"])
    );
    assert_eq!(
        result["commands"],
        json!([{ "name": "hello", "description": "Say hello", "completions": true }])
    );
    assert_eq!(
        result["shortcuts"],
        json!([{ "shortcut": "ctrl+shift+h", "description": "Hello" }])
    );
    assert_eq!(
        result["flags"],
        json!([{ "name": "plan", "description": "Plan mode", "type": "boolean", "default": false }])
    );
    assert_eq!(result["events"], json!(["session_start", "tool_call"]));
    assert_eq!(result["providers"][0]["name"], "local");
    assert_eq!(
        result["virtualModels"],
        json!([{ "provider": "router", "id": "auto" }])
    );
    assert_eq!(
        result["autocomplete"],
        json!([{ "triggerCharacters": ["@"] }])
    );

    host.request(
        1,
        "command.complete",
        json!({ "name": "hello", "prefix": "w" }),
    )
    .await;
    let response = host.expect_response(1).await;
    assert_eq!(response["result"], json!([{ "value": "wworld" }]));

    let params = json!({ "lines": ["see @fi"], "cursorLine": 0, "cursorCol": 7, "force": false });
    host.request(2, "autocomplete.suggest", params).await;
    let response = host.expect_response(2).await;
    assert_eq!(
        response["result"],
        json!({ "items": [{ "value": "@file" }], "prefix": "@fi" })
    );

    let params = json!({ "provider": "router", "id": "auto", "request": {} });
    host.request(3, "virtualModel.route", params).await;
    assert_eq!(
        host.expect_response(3).await["result"],
        json!({ "model": "router/auto" })
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn minimal_initialize_has_only_protocol_version() {
    let mut host = TestHost::start(Extension::new("empty"));
    assert_eq!(host.initialize().await, json!({ "protocolVersion": 1 }));
    host.finish().await.unwrap();
}

#[tokio::test]
async fn shutdown_runs_hook_and_exit_ends_serve() {
    let stopped = Arc::new(AtomicBool::new(false));
    let flag = stopped.clone();
    let ext = Extension::new("stop").on_shutdown(move |_| {
        let flag = flag.clone();
        async move {
            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });
    let mut host = TestHost::start(ext);
    host.initialize().await;
    host.request(5, "shutdown", json!({})).await;
    assert_eq!(host.expect_response(5).await["result"], Value::Null);
    assert!(stopped.load(Ordering::SeqCst));
    host.notify("exit", json!({})).await;
    host.join().await.unwrap();
}

#[tokio::test]
async fn eof_runs_shutdown_hook() {
    let stopped = Arc::new(AtomicBool::new(false));
    let flag = stopped.clone();
    let ext = Extension::new("eof").on_shutdown(move |host: Host| {
        let flag = flag.clone();
        async move {
            assert!(
                host.notify_info("bye").await.is_err(),
                "pi can no longer answer"
            );
            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
    });
    let mut host = TestHost::start(ext);
    host.initialize().await;
    host.finish().await.unwrap();
    assert!(stopped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn unknown_method_and_parse_error() {
    let mut host = TestHost::start(Extension::new("errors"));
    host.initialize().await;
    host.request(7, "nope", json!({})).await;
    assert_eq!(host.expect_response(7).await["error"]["code"], -32601);
    host.send(json!("not an object")).await;
    assert_eq!(host.recv().await["error"]["code"], -32600);
    host.finish().await.unwrap();
}

#[tokio::test]
async fn on_start_runs_after_initialized() {
    let ext = Extension::new("start").on_start(|host: Host| async move {
        let init = host.init().expect("init params");
        host.notify_info(format!("started in {}", init.cwd)).await
    });
    let mut host = TestHost::start(ext);
    host.initialize().await;
    let (id, params) = host.expect_request("ui.notify").await;
    assert_eq!(
        params,
        json!({ "message": "started in /work", "type": "info" })
    );
    host.answer(&id, Value::Null).await;
    host.finish().await.unwrap();
}
