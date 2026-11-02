//! The extension over the protocol: renderers of the built-in tools, the working indicator, and
//! the title of a run.

use pi_ext_compact_tui::{Config, extension};
use pi_ext_sdk::SPINNER;
use pi_ext_sdk::style::strip;
use pi_ext_sdk::test_util::TestHost;
use serde_json::{Value, json};

const ACCENT: &str = "\x1b[38;2;121;192;255m";

fn ctx() -> Value {
    json!({ "cwd": "/work/app", "hasUI": true, "flags": {}, "sessionName": "fix" })
}

fn host_answer(method: &str) -> Value {
    match method {
        "ui.getTheme" => json!({
            "name": "dark", "appearance": "dark", "mode": "truecolor",
            "fg": { "accent": ACCENT, "success": "\u{1b}[38;2;0;200;0m", "muted": "\u{1b}[38;5;245m" },
            "bg": { "toolSuccessBg": "\u{1b}[48;2;0;40;0m" }, "dim": []
        }),
        "keybindings.get" => json!({ "app.tools.expand": "ctrl+e", "app.interrupt": "escape" }),
        _ => Value::Null,
    }
}

async fn start() -> TestHost {
    let mut host = TestHost::start(extension(Config::default(), Some("/home/me".into())));
    let params = json!({
        "protocolVersion": 1, "piVersion": "0.13.0", "cwd": "/work/app", "agentDir": "/nonexistent",
        "extensionPath": "native:test/compact-tui", "platform": "linux", "mode": "tui", "hasUI": true,
        "flags": {}, "features": ["toolRenderers", "render.spinner", "render.refresh", "ui.getTheme", "theme_changed", "ui.working", "ui.chatStyle"]
    });
    host.request(0, "initialize", params).await;
    let init = host.expect_response(0).await["result"].clone();
    let renderers = init["toolRenderers"].as_array().unwrap();
    assert_eq!(renderers.len(), 7, "{init}");
    assert_eq!(
        renderers[0],
        json!({ "toolName": "bash", "renderCall": true, "renderResult": true, "renderShell": "self" })
    );
    assert!(init.get("tools").is_none(), "registers no tools: {init}");
    host.notify("initialized", json!({})).await;
    let event =
        json!({ "name": "session_start", "event": { "type": "session_start" }, "ctx": ctx() });
    let mut asked: Vec<(String, Value)> = Vec::new();
    host.drive(1, "event", event, |method, params| {
        asked.push((method.into(), params.clone()));
        host_answer(method)
    })
    .await;
    // `on_start` reads the theme and the keys, then sets the chat style, in its own task.
    let label = "ui.setHiddenThinkingLabel";
    while !asked.iter().any(|(method, _)| method == label) {
        let message = host.recv().await;
        if let Some(method) = message["method"]
            .as_str()
            .filter(|_| !message["id"].is_null())
        {
            host.answer(&message["id"].clone(), host_answer(method))
                .await;
            asked.push((method.into(), message["params"].clone()));
        }
    }
    let params = |name: &str| {
        asked
            .iter()
            .find(|(method, _)| method == name)
            .unwrap()
            .1
            .clone()
    };
    let style = params("ui.setChatStyle");
    assert_eq!(strip(style["userPrefix"].as_str().unwrap()), "› ");
    assert!(
        style["assistantPrefix"]
            .as_str()
            .unwrap()
            .starts_with(ACCENT),
        "{style}"
    );
    assert_eq!(
        (&style["collapseThinking"], &style["roundedEditor"]),
        (&json!(true), &json!(true))
    );
    assert_eq!(params(label)["label"], "∴ Thought · ctrl+t to show");
    let probe = json!({ "toolName": "bash", "result": { "content": [{ "type": "text", "text": "a\nb\nc\nd\ne\nf\ng\nh" }] },
        "options": {}, "context": {}, "width": 60 });
    for id in 100..600 {
        let lines = render(&mut host, id, "render.toolResult", probe.clone()).await;
        if lines.to_string().contains("ctrl+e") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    host
}

async fn render(host: &mut TestHost, id: u64, method: &str, params: Value) -> Value {
    host.request(id, method, params).await;
    host.expect_response(id).await["result"].clone()
}

#[tokio::test]
async fn built_in_tools_get_compact_rows() {
    let mut host = start().await;
    let running = json!({ "argsComplete": true, "executionStarted": true, "isPartial": true, "cwd": "/work/app" });
    let call = json!({ "toolName": "bash", "toolCallId": "c1", "args": { "command": "cargo test\necho" }, "context": running, "width": 40 });
    let answer = render(&mut host, 2, "render.toolCall", call).await;
    let line = answer["lines"][0].as_str().unwrap();
    assert!(line.starts_with(&format!("{ACCENT}{SPINNER}")), "{line:?}");
    assert_eq!(strip(line).trim_start_matches(SPINNER), " cargo test …");
    assert_eq!(answer["refreshMs"], json!(1000));

    let done = json!({ "argsComplete": true, "executionStarted": true, "isPartial": false, "cwd": "/work/app" });
    let read = json!({ "toolName": "read", "args": { "path": "/work/app/src/lib.rs", "offset": 10, "limit": 5 }, "context": done, "width": 60 });
    let answer = render(&mut host, 3, "render.toolCall", read).await;
    assert_eq!(
        strip(answer["lines"][0].as_str().unwrap()),
        "→ Read src/lib.rs:10-14"
    );
    assert!(answer.get("refreshMs").is_none(), "{answer}");

    let output: String = (1..=9).map(|n| format!("line {n}\n")).collect();
    let result = json!({ "toolName": "bash", "result": { "content": [{ "type": "text", "text": output }] },
        "options": { "isPartial": false }, "context": done, "width": 40 });
    let lines = render(&mut host, 4, "render.toolResult", result).await["lines"].clone();
    let plain: Vec<String> = lines
        .as_array()
        .unwrap()
        .iter()
        .map(|line| strip(line.as_str().unwrap()))
        .collect();
    assert_eq!(
        plain,
        [
            "  ⎿ … +4 lines (ctrl+e to expand)",
            "    line 5",
            "    line 6",
            "    line 7",
            "    line 8",
            "    line 9"
        ]
    );

    let diff = json!({ "diff": " 1 a\n-2 b\n+2 c", "patch": "" });
    let edit = json!({ "toolName": "edit", "result": { "content": [], "details": diff }, "options": {},
        "context": { "args": { "path": "a.rs", "edits": [{}] } }, "width": 30 });
    let lines = render(&mut host, 5, "render.toolResult", edit).await["lines"].clone();
    let plain: Vec<String> = lines
        .as_array()
        .unwrap()
        .iter()
        .map(|line| strip(line.as_str().unwrap()))
        .collect();
    assert_eq!(plain[0], "  ⎿ 1 added, 1 removed");
    assert_eq!(plain[3].trim_end(), "    2 + c");
    assert!(
        lines[3].as_str().unwrap().contains("\x1b[48;2;0;40;0m"),
        "{lines}"
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn a_run_shows_the_working_indicator_and_the_busy_title() {
    let mut host = start().await;
    let mut seen: Vec<(String, Value)> = Vec::new();
    let event = json!({ "name": "agent_start", "event": { "type": "agent_start" }, "ctx": ctx() });
    host.drive(6, "event", event, |method, params| {
        seen.push((method.into(), params.clone()));
        Value::Null
    })
    .await;
    let methods: Vec<&str> = seen.iter().map(|(method, _)| method.as_str()).collect();
    assert_eq!(
        methods,
        [
            "ui.setWidget",
            "ui.setTitle",
            "ui.setWorkingIndicator",
            "ui.setWorkingMessage"
        ]
    );
    assert_eq!(seen[1].1["title"], "● π - fix - app");
    let frames = seen[2].1["frames"].as_array().unwrap();
    assert_eq!(strip(frames[0].as_str().unwrap()), "⠋ Thinking…");
    assert_eq!(seen[2].1["intervalMs"], json!(80));
    assert_eq!(
        strip(seen[3].1["message"].as_str().unwrap()),
        "(0s · esc to interrupt)"
    );

    seen.clear();
    let tool = json!({ "type": "tool_execution_start", "toolCallId": "t1", "toolName": "edit", "args": { "path": "src/main.rs" } });
    let event = json!({ "name": "tool_execution_start", "event": tool, "ctx": ctx() });
    host.drive(7, "event", event, |method, params| {
        seen.push((method.into(), params.clone()));
        Value::Null
    })
    .await;
    let frame = seen[0].1["frames"][0].as_str().unwrap();
    assert_eq!(strip(frame), "⠋ Editing main.rs…");

    seen.clear();
    let end =
        json!({ "type": "agent_end", "messages": [{ "role": "assistant", "stopReason": "stop" }] });
    let event = json!({ "name": "agent_end", "event": end, "ctx": ctx() });
    host.drive(8, "event", event, |method, params| {
        seen.push((method.into(), params.clone()));
        Value::Null
    })
    .await;
    let expected = [
        ("ui.setWorkingIndicator".to_string(), json!({})),
        ("ui.setWorkingMessage".to_string(), json!({})),
        (
            "ui.setTitle".to_string(),
            json!({ "title": "π - fix - app" }),
        ),
    ];
    assert_eq!(seen, expected);
    host.finish().await.unwrap();
}
