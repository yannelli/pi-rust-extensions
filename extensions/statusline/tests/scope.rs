mod common;

use common::TestHost;
use pi_ext_e2e::TempDir;
use pi_ext_statusline::{extension, sources};
use serde_json::{Value, json};

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
        let (panel_id, panel) = host.expect_request("ui.custom").await;
        let surface = panel["surface"].clone();
        for key in ["\r", "off", "\r"] {
            host.notify("ui.key", json!({"surface": surface, "data": key}))
                .await;
            loop {
                let message = host.recv().await;
                match message["method"].as_str() {
                    Some("ui.update") => break,
                    Some("settings.get") => host.answer(&message["id"], Value::Null).await,
                    _ => panic!("unexpected message {message}"),
                }
            }
        }
        host.notify("ui.key", json!({"surface": surface, "data": "\u{1b}"}))
            .await;
        let closed = host.recv().await;
        assert_eq!(closed["method"], "ui.close");
        host.answer(&panel_id, Value::Null).await;
        host.expect_response(1).await;
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
