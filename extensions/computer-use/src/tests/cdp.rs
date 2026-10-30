use std::sync::{Arc, Mutex};

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use pi_ext_sdk::CancellationToken;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

use crate::act::UiAction;
use crate::outline::Outline;
use crate::state::{Capture, Observation, Target};
use crate::{page, page_act};

const JPEG: [u8; 22] = [
    0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xC0, 0, 17, 8, 0, 200, 1, 44, 3, 0, 0, 0, 0,
];

fn reply(request: &Value) -> Value {
    let expression = request["params"]["expression"].as_str().unwrap_or_default();
    match request["method"].as_str().unwrap_or_default() {
        "Runtime.evaluate" if expression.contains("MAX_NODES") => json!({ "result": { "value": {
            "title": "Shop", "url": "https://a.test/", "viewport": { "w": 800, "h": 600 },
            "root": { "role": "web_area", "label": "Shop", "path": [], "children": [
                { "role": "button", "label": "Buy", "canPress": true, "path": [0],
                  "frame": { "x": 10, "y": 10, "w": 50, "h": 20 }, "children": [] }
            ] }
        } } }),
        "Runtime.evaluate" => json!({ "result": { "value": true } }),
        "Page.captureScreenshot" => {
            json!({ "data": base64::engine::general_purpose::STANDARD.encode(JPEG) })
        }
        _ => json!({}),
    }
}

/// Serves one page socket, records each request, and answers by method.
async fn mock_page(log: Arc<Mutex<Vec<Value>>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                while let Some(Ok(Message::Text(text))) = socket.next().await {
                    let request: Value = serde_json::from_str(text.as_str()).unwrap();
                    log.lock().unwrap().push(request.clone());
                    socket
                        .send(Message::Text(
                            json!({ "method": "Page.frameNavigated", "params": {} })
                                .to_string()
                                .into(),
                        ))
                        .await
                        .unwrap();
                    let answer = json!({ "id": request["id"], "result": reply(&request) });
                    socket
                        .send(Message::Text(answer.to_string().into()))
                        .await
                        .unwrap();
                }
            });
        }
    });
    port
}

#[tokio::test]
async fn page_look_and_act_over_cdp() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let port = mock_page(log.clone()).await;
    let cancel = CancellationToken::new();
    let target = Target {
        app: "Chrome (managed)".into(),
        page_id: Some("P1".into()),
        cdp_port: Some(port),
        ..Target::default()
    };
    let (looked, root, image) = page::look(&target, "fused", &cancel).await.unwrap();
    assert_eq!(
        (looked.window_title.as_str(), looked.frame_points.w),
        ("Shop", 800.0)
    );
    assert_eq!(root.find("@e2").unwrap().label.as_deref(), Some("Buy"));
    let image = image.unwrap();
    assert_eq!((image.width, image.height), (300, 200));

    let observation = Observation {
        target: looked,
        capture: Capture {
            state_id: "b1".into(),
            width: 300,
            height: 200,
            ..Capture::default()
        },
        outline: Outline {
            look_id: "l".into(),
            root,
        },
    };
    let actions: Vec<UiAction> = serde_json::from_value(json!([
        { "action": "press", "ref": "@e2" },
        { "action": "typeText", "text": "hi" },
        { "action": "keypress", "keys": ["cmd+a"] },
        { "action": "click", "x": 150, "y": 100 }
    ]))
    .unwrap();
    let outcomes = page_act::act(&observation, &actions, &cancel)
        .await
        .unwrap();
    assert!(
        outcomes.iter().all(|o| o["outcome"] == "worked"),
        "{outcomes:?}"
    );

    let log = log.lock().unwrap();
    let methods: Vec<&str> = log.iter().map(|r| r["method"].as_str().unwrap()).collect();
    assert_eq!(
        &methods[..2],
        ["Runtime.evaluate", "Page.captureScreenshot"]
    );
    assert_eq!(log[2]["params"]["expression"], "window.__piCu.press(0)");
    assert_eq!(log[3]["params"], json!({ "text": "hi" }));
    assert_eq!(
        (
            log[4]["params"]["key"].as_str(),
            log[4]["params"]["modifiers"].as_u64()
        ),
        (Some("a"), Some(4))
    );
    assert_eq!(log[4]["params"]["text"], Value::Null);
    let press = log
        .iter()
        .find(|r| r["params"]["type"] == "mousePressed")
        .unwrap();
    assert_eq!(
        (press["params"]["x"].as_f64(), press["params"]["y"].as_f64()),
        (Some(400.0), Some(300.0))
    );
}

#[tokio::test]
async fn stale_browser_ref_fails_before_cdp() {
    let cancel = CancellationToken::new();
    let observation = Observation {
        target: Target {
            page_id: Some("P1".into()),
            cdp_port: Some(1),
            ..Target::default()
        },
        capture: Capture::default(),
        outline: Outline::default(),
    };
    let actions: Vec<UiAction> =
        serde_json::from_value(json!([{ "action": "press", "ref": "@e1" }])).unwrap();
    let error = page_act::act(&observation, &actions, &cancel)
        .await
        .unwrap_err();
    assert!(
        error.message().starts_with("Outline ref '@e1' is stale"),
        "{}",
        error.message()
    );
}

#[tokio::test]
async fn navigation_reports_a_load_timeout() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let port = mock_page(log.clone()).await;
    let target = Target {
        page_id: Some("P1".into()),
        cdp_port: Some(port),
        ..Target::default()
    };
    let error = page::navigate_with_timeout(
        &target,
        "https://example.test/",
        &CancellationToken::new(),
        std::time::Duration::from_millis(30),
    )
    .await
    .unwrap_err();
    assert!(error.message().contains("did not finish loading"));
    let log = log.lock().unwrap();
    assert_eq!(log[0]["method"], "Page.navigate");
    assert_eq!(log[1]["params"]["expression"], "document.readyState");
}
