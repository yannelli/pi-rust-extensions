mod common;

use common::TestHost;
use pi_ext_e2e::TempDir;
use pi_ext_statusline::{extension, sources};
use serde_json::{Value, json};

async fn next_panel_message(host: &mut TestHost) -> Value {
    loop {
        let message = host.recv().await;
        match message["method"].as_str() {
            Some("settings.get" | "ui.setFooter") => host.answer(&message["id"], Value::Null).await,
            _ => return message,
        }
    }
}

async fn key(host: &mut TestHost, surface: &Value, data: &str) -> Value {
    host.notify("ui.key", json!({"surface": surface, "data": data}))
        .await;
    let message = next_panel_message(host).await;
    assert_eq!(message["method"], "ui.update", "{message}");
    message
}

#[tokio::test]
async fn project_badge_colors_edit_inherited_badges_without_copying_global_settings() {
    let root = TempDir::new().unwrap();
    let agent = root.path().join("agent");
    let cwd = root.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let global_path = sources::global_config_path(&agent);
    let project_path = sources::project_config_path(&cwd);
    let global = json!({"type": "builtin", "chip": "block", "theme": "ocean",
        "command": "saved.sh", "segments": [[{"text": "ALPHA", "unknown": true}, {"text": "BETA"}]]});
    let project = json!({"theme": "forest", "projectKey": true});
    sources::write_config(&global_path, &global).unwrap();
    sources::write_config(&project_path, &project).unwrap();
    let mut host = TestHost::start(extension(root.path().join("home")));
    host.initialize_in(cwd.to_str().unwrap(), agent.to_str().unwrap())
        .await;
    host.request(
        1,
        "command.run",
        json!({"name": "statusline", "args": "edit",
        "ctx": {"cwd": cwd, "hasUI": true, "isProjectTrusted": true}}),
    )
    .await;
    let (id, _) = host.expect_request("ui.select").await;
    host.answer(&id, json!("Project")).await;
    let panel = next_panel_message(&mut host).await;
    assert_eq!(panel["method"], "ui.custom");
    let surface = panel["params"]["surface"].clone();
    let mut selected = false;
    for _ in 0..20 {
        let message = key(&mut host, &surface, "\x1b[B").await;
        selected = message["params"]["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line.as_str().unwrap_or_default().contains("→ Badge colors"));
        if selected {
            break;
        }
    }
    assert!(selected, "Badge colors must be reachable");
    let badges = key(&mut host, &surface, "\r").await;
    let lines = badges["params"]["lines"].to_string();
    assert!(lines.contains("ALPHA") && lines.contains("BETA"), "{lines}");
    for data in [
        "\r", "\r", "#ffffff", "\r", "\x1b", "\x1b[B", "\x1b[B", "\r",
    ] {
        key(&mut host, &surface, data).await;
    }
    let saved = Value::Object(sources::read_object(&project_path).unwrap());
    assert_eq!(
        saved,
        json!({"type": "builtin", "theme": "forest", "projectKey": true,
        "segments": [[{"text": "ALPHA", "unknown": true, "foreground": "#ffffff"}, {"text": "BETA"}]]})
    );
    assert_eq!(
        Value::Object(sources::read_object(&global_path).unwrap()),
        global
    );
    host.notify("ui.key", json!({"surface": surface, "data": "\u{1b}"}))
        .await;
    assert_eq!(next_panel_message(&mut host).await["method"], "ui.close");
    host.answer(&panel["id"], Value::Null).await;
    assert_eq!(next_panel_message(&mut host).await["id"], 1);
    host.finish().await.unwrap();
}
