mod common;

use std::path::{Path, PathBuf};

use common::TestHost;
use pi_ext_sdk::style::{strip, visible_width};
use pi_ext_statusline::extension;
use serde_json::{Value, json};

struct Dirs {
    root: PathBuf,
}

impl Dirs {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("pi-statusline-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("agent/ext")).unwrap();
        std::fs::create_dir_all(root.join("node-cli/.pi/ext")).unwrap();
        Self { root }
    }

    fn agent(&self) -> PathBuf {
        self.root.join("agent")
    }

    fn cwd(&self) -> PathBuf {
        self.root.join("node-cli")
    }

    fn write(&self, path: &Path, value: Value) {
        std::fs::write(path, value.to_string()).unwrap();
    }
}

impl Drop for Dirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn ctx(cwd: &Path, trusted: bool) -> Value {
    json!({
        "cwd": cwd, "hasUI": true, "flags": {}, "isProjectTrusted": trusted,
        "model": { "provider": "openai", "id": "gpt-5", "contextWindow": 400000, "reasoning": true },
        "thinkingLevel": "high",
        "footer": {
            "gitBranch": "main", "runState": "working",
            "sessionStats": { "input": 1200, "output": 300, "cacheRead": 0, "cacheWrite": 0, "cost": 0.5,
                "contextTokens": 40000, "contextWindow": 400000, "contextPercent": 10.0 }
        }
    })
}

#[derive(Default)]
struct Seen {
    footers: Vec<Vec<String>>,
    warnings: Vec<String>,
    responded: bool,
}

/// Answers host requests until `done` holds.
async fn pump(host: &mut TestHost, seen: &mut Seen, event_id: u64, done: impl Fn(&Seen) -> bool) {
    while !done(seen) {
        let message = host.recv().await;
        let id = message["id"].clone();
        match message["method"].as_str() {
            Some("settings.get") => host.answer(&id, Value::Null).await,
            Some("exec") => {
                host.answer(
                    &id,
                    json!({"stdout": "", "stderr": "no git", "code": 128, "killed": false}),
                )
                .await
            }
            Some("ui.setFooter") => {
                let lines = message["params"]["lines"].as_array().unwrap();
                seen.footers.push(
                    lines
                        .iter()
                        .map(|l| l.as_str().unwrap().to_string())
                        .collect(),
                );
                host.answer(&id, Value::Null).await;
            }
            Some("ui.notify") => {
                seen.warnings
                    .push(message["params"]["message"].as_str().unwrap().to_string());
                host.answer(&id, Value::Null).await;
            }
            None if id == event_id => seen.responded = true,
            _ => panic!("unexpected message {message}"),
        }
    }
}

#[tokio::test]
async fn footer_lines_render_for_the_snapshot_and_follow_the_width() {
    let dirs = Dirs::new("render");
    let config = json!({"chip": "plain", "icons": "none", "segments": ["model", "thinking", "fill", "dir", "branch"]});
    dirs.write(&dirs.agent().join("ext/statusline.json"), config);
    let mut host = TestHost::start(extension(PathBuf::from("/nonexistent-home")));
    host.initialize_in(dirs.cwd().to_str().unwrap(), dirs.agent().to_str().unwrap())
        .await;
    let event = json!({ "name": "session_start", "event": { "type": "session_start" }, "ctx": ctx(&dirs.cwd(), false) });
    host.request(1, "event", event).await;
    let mut seen = Seen::default();
    pump(&mut host, &mut seen, 1, |seen| {
        seen.responded && !seen.footers.is_empty()
    })
    .await;
    let line = &seen.footers[0][0];
    assert_eq!(visible_width(line), 80, "{line:?}");
    let text = strip(line);
    assert!(text.starts_with("gpt-5 • high "), "{text:?}");
    assert!(text.ends_with("node-cli • main"), "{text:?}");

    host.notify("ui.resize", json!({ "surface": "footer", "width": 40 }))
        .await;
    let count = seen.footers.len();
    pump(&mut host, &mut seen, 1, |seen| seen.footers.len() > count).await;
    let narrow = seen.footers.last().unwrap()[0].clone();
    assert_eq!(visible_width(&narrow), 40, "{narrow:?}");
    assert!(strip(&narrow).ends_with("node-cli • main"));
    host.finish().await.unwrap();
}

#[tokio::test]
async fn an_untrusted_project_cannot_switch_to_a_command() {
    let dirs = Dirs::new("trust");
    dirs.write(
        &dirs.agent().join("ext/statusline.json"),
        json!({"chip": "plain", "icons": "none", "segments": ["model"]}),
    );
    let marker = dirs.root.join("ran");
    let command = format!("touch {}; echo evil", marker.display());
    dirs.write(
        &dirs.cwd().join(".pi/ext/statusline.json"),
        json!({"type": "command", "command": command}),
    );
    let mut host = TestHost::start(extension(PathBuf::from("/nonexistent-home")));
    host.initialize_in(dirs.cwd().to_str().unwrap(), dirs.agent().to_str().unwrap())
        .await;
    let event = json!({ "name": "session_start", "event": { "type": "session_start" }, "ctx": ctx(&dirs.cwd(), false) });
    host.request(1, "event", event).await;
    let mut seen = Seen::default();
    pump(&mut host, &mut seen, 1, |seen| {
        seen.responded && !seen.footers.is_empty() && !seen.warnings.is_empty()
    })
    .await;
    assert_eq!(strip(&seen.footers[0][0]), "gpt-5");
    assert!(
        seen.warnings[0].contains("not trusted"),
        "{:?}",
        seen.warnings
    );
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!marker.exists());
    host.finish().await.unwrap();
}

#[tokio::test]
async fn a_choice_shows_before_any_event_arrives() {
    let dirs = Dirs::new("choice");
    let mut host = TestHost::start(extension(PathBuf::from("/nonexistent-home")));
    host.initialize_in(dirs.cwd().to_str().unwrap(), dirs.agent().to_str().unwrap())
        .await;
    let params = json!({ "name": "statusline", "args": "minimal", "ctx": ctx(&dirs.cwd(), false) });
    host.request(1, "command.run", params).await;
    let mut seen = Seen::default();
    let shown = |seen: &Seen| seen.responded && !seen.footers.is_empty();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        pump(&mut host, &mut seen, 1, shown),
    )
    .await
    .expect("the chosen status line reaches the footer");
    assert!(
        strip(&seen.footers[0][0]).contains("gpt-5"),
        "{:?}",
        seen.footers
    );
    host.finish().await.unwrap();
}

#[tokio::test]
async fn the_footer_follows_the_snapshot_mode_over_the_load_time_mode() {
    let dirs = Dirs::new("load-mode");
    dirs.write(
        &dirs.agent().join("ext/statusline.json"),
        json!({"chip": "plain", "icons": "none", "segments": ["model"]}),
    );
    let mut host = TestHost::start(extension(PathBuf::from("/nonexistent-home")));
    host.initialize_as(
        dirs.cwd().to_str().unwrap(),
        dirs.agent().to_str().unwrap(),
        "print",
        false,
    )
    .await;
    let mut ctx = ctx(&dirs.cwd(), false);
    ctx["mode"] = json!("tui");
    let event =
        json!({ "name": "session_start", "event": { "type": "session_start" }, "ctx": ctx });
    host.request(1, "event", event).await;
    let mut seen = Seen::default();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        pump(&mut host, &mut seen, 1, |seen| {
            seen.responded && !seen.footers.is_empty()
        }),
    )
    .await
    .expect("the footer renders in tui mode");
    assert_eq!(strip(&seen.footers[0][0]), "gpt-5");
    host.finish().await.unwrap();
}
