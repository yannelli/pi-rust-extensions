//! Theme tokens use pi's theme colors from `ui.getTheme` and follow `theme.changed`. The theme
//! is process-wide, so these tests live in their own test binary.

mod common;

use std::path::PathBuf;

use common::TestHost;
use pi_ext_statusline::colors::{self, ColorMode, ThemeColor};
use pi_ext_statusline::extension;
use serde_json::{Value, json};

const ACCENT: &str = "\x1b[38;2;121;192;255m";
const LIGHT_ACCENT: &str = "\x1b[38;2;9;105;218m";

fn theme(accent: &str, mode: &str) -> Value {
    json!({
        "name": "github", "appearance": "dark", "mode": mode,
        "fg": { "accent": accent, "dim": "\u{1b}[38;5;245m", "text": "\u{1b}[39m" },
        "bg": {}, "dim": ["dim"]
    })
}

/// Answers host requests until a footer arrives; returns its first line.
async fn next_footer(host: &mut TestHost, theme: &Value) -> String {
    loop {
        let message = host.recv().await;
        let id = message["id"].clone();
        match message["method"].as_str() {
            Some("ui.getTheme") => host.answer(&id, theme.clone()).await,
            Some("settings.get") | Some("ui.notify") => host.answer(&id, Value::Null).await,
            Some("exec") => {
                host.answer(&id, json!({ "stdout": "", "stderr": "", "code": 128 }))
                    .await
            }
            Some("ui.setFooter") => {
                host.answer(&id, Value::Null).await;
                return message["params"]["lines"][0].as_str().unwrap().to_string();
            }
            Some(other) if !id.is_null() => panic!("unexpected request {other}"),
            _ => {}
        }
    }
}

#[tokio::test]
async fn footer_tokens_use_the_pi_theme_and_follow_theme_changes() {
    assert_eq!(ThemeColor::Accent.paint("a"), "\x1b[36ma\x1b[39m");
    let dir = std::env::temp_dir().join(format!("pi-statusline-theme-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("agent/ext")).unwrap();
    let config =
        json!({ "chip": "plain", "icons": "none", "theme": "terminal", "segments": ["model"] });
    std::fs::write(dir.join("agent/ext/statusline.json"), config.to_string()).unwrap();
    let mut host = TestHost::start(extension(PathBuf::from("/nonexistent-home")));
    let params = json!({
        "protocolVersion": 1, "piVersion": "0.13.0", "cwd": dir, "agentDir": dir.join("agent"),
        "extensionPath": "native:test/test", "platform": "linux", "mode": "tui", "hasUI": true,
        "flags": {}, "features": ["ui.getTheme", "theme_changed"]
    });
    host.request(0, "initialize", params).await;
    let init = host.expect_response(0).await;
    assert!(
        init["result"]["events"]
            .as_array()
            .unwrap()
            .contains(&json!("theme_changed")),
        "{init}"
    );
    host.notify("initialized", json!({})).await;
    let ctx = json!({ "cwd": dir, "hasUI": true, "flags": {}, "model": { "provider": "openai", "id": "gpt-5" } });
    let event =
        json!({ "name": "session_start", "event": { "type": "session_start" }, "ctx": ctx });
    host.request(1, "event", event).await;

    let dark = theme(ACCENT, "truecolor");
    let mut line = next_footer(&mut host, &dark).await;
    while !line.contains(ACCENT) {
        line = next_footer(&mut host, &dark).await;
    }
    assert!(
        line.contains(&format!("{ACCENT}gpt-5")) && !line.contains("\x1b[36m"),
        "{line:?}"
    );
    assert_eq!(
        ThemeColor::Dim.paint("d"),
        "\x1b[38;5;245m\x1b[2md\x1b[22m\x1b[39m"
    );
    assert_eq!(ThemeColor::Accent.rgb(), [121, 192, 255]);

    host.notify(
        "theme.changed",
        json!({ "theme": theme(LIGHT_ACCENT, "256color") }),
    )
    .await;
    let mut line = next_footer(&mut host, &dark).await;
    while !line.contains(LIGHT_ACCENT) {
        line = next_footer(&mut host, &dark).await;
    }
    assert_eq!(colors::mode(), ColorMode::Ansi256);
    let _ = std::fs::remove_dir_all(&dir);
}
