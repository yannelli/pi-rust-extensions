mod common;

use common::TestHost;
use pi_ext_e2e::TempDir;
use pi_ext_statusline::{extension, sources};
use serde_json::{Value, json};

/// The next message that is not a config read or a footer preview, which this answers.
async fn next_panel_message(host: &mut TestHost) -> Value {
    loop {
        let message = host.recv().await;
        match message["method"].as_str() {
            Some("settings.get" | "ui.setFooter") => host.answer(&message["id"], Value::Null).await,
            _ => return message,
        }
    }
}

async fn editor_scope(trusted: bool, selected: Value) {
    let root = TempDir::new().unwrap();
    let agent = root.path().join("agent");
    let cwd = root.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let global_path = sources::global_config_path(&agent);
    let project_path = sources::project_config_path(&cwd);
    let original = json!({"type": "builtin", "preset": "compact"});
    sources::write_config(&global_path, &original).unwrap();
    let mut host = TestHost::start(extension(root.path().join("home")));
    host.initialize_in(cwd.to_str().unwrap(), agent.to_str().unwrap())
        .await;
    host.request(
        1,
        "command.run",
        json!({
            "name": "statusline", "args": "edit",
            "ctx": {"cwd": cwd, "hasUI": true, "isProjectTrusted": trusted}
        }),
    )
    .await;
    let (id, params) = host.expect_request("ui.select").await;
    assert_eq!(
        params["options"],
        if trusted {
            json!(["Global", "Project"])
        } else {
            json!(["Global"])
        }
    );
    host.answer(&id, selected.clone()).await;
    if selected.is_null() || (selected == "Project" && !trusted) {
        host.expect_response(1).await;
    } else {
        let panel_message = next_panel_message(&mut host).await;
        assert_eq!(panel_message["method"], "ui.custom");
        let (panel_id, panel) = (panel_message["id"].clone(), panel_message["params"].clone());
        let surface = panel["surface"].clone();
        for key in ["\r", "off", "\r"] {
            host.notify("ui.key", json!({"surface": surface, "data": key}))
                .await;
            let message = next_panel_message(&mut host).await;
            assert_eq!(
                message["method"], "ui.update",
                "unexpected message {message}"
            );
        }
        host.notify("ui.key", json!({"surface": surface, "data": "\u{1b}"}))
            .await;
        let closed = next_panel_message(&mut host).await;
        assert_eq!(closed["method"], "ui.close");
        host.answer(&panel_id, Value::Null).await;
        let done = next_panel_message(&mut host).await;
        assert_eq!(done["id"], 1, "unexpected message {done}");
    }
    let global = Value::Object(sources::read_object(&global_path).unwrap());
    if trusted && selected == "Project" {
        assert_eq!(global, original);
        assert_eq!(
            Value::Object(sources::read_object(&project_path).unwrap()),
            json!({"type": "off"})
        );
    } else {
        assert!(!project_path.exists());
        assert_eq!(
            global,
            if selected == "Global" {
                json!({"type": "off", "preset": "compact"})
            } else {
                original
            }
        );
    }
    host.finish().await.unwrap();
}

#[tokio::test]
async fn trusted_editor_writes_the_project_layer() {
    editor_scope(true, json!("Project")).await;
}

#[tokio::test]
async fn untrusted_editor_writes_global() {
    editor_scope(false, json!("Global")).await;
}

#[tokio::test]
async fn untrusted_editor_rejects_a_project_selection() {
    editor_scope(false, json!("Project")).await;
}

#[tokio::test]
async fn canceling_scope_leaves_config_unchanged() {
    editor_scope(true, Value::Null).await;
}
