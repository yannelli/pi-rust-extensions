mod common;

use common::TestHost;
use pi_ext_context_include::extension;
use serde_json::{Value, json};

fn ctx(cwd: &str) -> Value {
    json!({ "cwd": cwd, "hasUI": true, "flags": {} })
}

fn before_agent_start(files: Value) -> Value {
    json!({
        "type": "before_agent_start", "prompt": "hi",
        "systemPromptOptions": { "contextFiles": files, "sections": {} }
    })
}

#[tokio::test]
async fn session_start_then_before_agent_start_sets_section() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path())
        .unwrap()
        .display()
        .to_string();
    std::fs::write(format!("{root}/notes.md"), "NOTE").unwrap();
    std::fs::write(format!("{root}/big.md"), "0123456789").unwrap();
    let agents = format!("{root}/AGENTS.md");
    std::fs::write(&agents, "@notes.md\n@big.md").unwrap();

    let mut host = TestHost::start(extension());
    let init = host.initialize().await;
    assert_eq!(
        init["events"],
        json!(["session_start", "before_agent_start"])
    );

    host.request(
        1,
        "command.run",
        json!({ "name": "context-include:status", "args": "", "ctx": ctx(&root) }),
    )
    .await;
    let (id, params) = host.expect_request("ui.notify").await;
    assert!(
        params["message"]
            .as_str()
            .unwrap()
            .contains("No scan data yet")
    );
    host.answer(&id, Value::Null).await;
    host.expect_response(1).await;

    let start = json!({ "type": "session_start", "reason": "startup" });
    host.request(
        2,
        "event",
        json!({ "name": "session_start", "event": start, "ctx": ctx(&root) }),
    )
    .await;
    let (id, params) = host.expect_request("settings.get").await;
    assert_eq!(params, json!({ "namespace": "contextInclude" }));
    host.answer(&id, json!({ "maxBytes": 8 })).await;
    assert_eq!(host.expect_response(2).await["result"], json!({}));

    let files = json!([{ "path": agents, "content": "@notes.md\n@big.md" }]);
    let event = before_agent_start(files);
    host.request(
        3,
        "event",
        json!({ "name": "before_agent_start", "event": event, "ctx": ctx(&root) }),
    )
    .await;
    let response = host.expect_response(3).await;
    let section = format!(
        "Additional files included via @-syntax references in the project instructions:\n\n\
         <project_instructions path=\"{root}/notes.md\">\nNOTE\n</project_instructions>"
    );
    assert_eq!(
        response["result"],
        json!({ "patch": { "systemPromptOptions": { "sections": { "context_include": section } } } })
    );

    host.request(
        4,
        "command.run",
        json!({ "name": "context-include:status", "args": "", "ctx": ctx(&root) }),
    )
    .await;
    let (id, params) = host.expect_request("ui.notify").await;
    assert_eq!(params["type"], "info");
    let message = params["message"].as_str().unwrap();
    assert!(message.contains("maxBytes: 8 (0 KB)"), "{message}");
    assert!(
        message.contains(&format!("  4 B  {root}/notes.md")),
        "{message}"
    );
    assert!(
        message.contains(&format!(
            "  size limit exceeded (would reach 14 bytes)  @big.md  → {root}/big.md"
        )),
        "{message}"
    );
    host.answer(&id, Value::Null).await;
    host.expect_response(4).await;

    let empty = before_agent_start(json!([{ "path": agents, "content": "no refs" }]));
    host.request(
        5,
        "event",
        json!({ "name": "before_agent_start", "event": empty, "ctx": ctx(&root) }),
    )
    .await;
    assert_eq!(
        host.expect_response(5).await["result"],
        json!({ "patch": { "systemPromptOptions": { "sections": { "context_include": null } } } })
    );
    host.finish().await.unwrap();
}

fn trusted_ctx(cwd: &str) -> Value {
    let mut ctx = ctx(cwd);
    ctx["isProjectTrusted"] = json!(true);
    ctx
}

#[tokio::test]
async fn project_file_replaces_setting() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path())
        .unwrap()
        .display()
        .to_string();
    std::fs::create_dir_all(format!("{root}/.pi/ext")).unwrap();
    std::fs::write(
        format!("{root}/.pi/ext/context-include.json"),
        r#"{"maxDepth": 2}"#,
    )
    .unwrap();

    let mut host = TestHost::start(extension());
    host.initialize().await;
    let start = json!({ "type": "session_start", "reason": "startup" });
    host.request(
        1,
        "event",
        json!({ "name": "session_start", "event": start, "ctx": trusted_ctx(&root) }),
    )
    .await;
    let (id, _) = host.expect_request("settings.get").await;
    host.answer(&id, json!({ "maxDepth": 7, "maxBytes": 9 }))
        .await;
    host.expect_response(1).await;

    host.request(
        2,
        "command.run",
        json!({ "name": "context-include:status", "args": "", "ctx": ctx(&root) }),
    )
    .await;
    let (id, params) = host.expect_request("ui.notify").await;
    let message = params["message"].as_str().unwrap();
    assert!(
        message.contains("maxDepth: 2\nmaxBytes: 500,000 (488 KB)"),
        "{message}"
    );
    host.answer(&id, Value::Null).await;
    host.expect_response(2).await;
    host.finish().await.unwrap();
}

#[tokio::test]
async fn untrusted_project_file_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path())
        .unwrap()
        .display()
        .to_string();
    std::fs::create_dir_all(format!("{root}/.pi/ext")).unwrap();
    std::fs::write(
        format!("{root}/.pi/ext/context-include.json"),
        r#"{"maxDepth": 2, "allowedRoots": ["/"]}"#,
    )
    .unwrap();

    let mut host = TestHost::start(extension());
    host.initialize().await;
    let mut untrusted = ctx(&root);
    untrusted["isProjectTrusted"] = json!(false);
    let start = json!({ "type": "session_start", "reason": "startup" });
    host.request(
        1,
        "event",
        json!({ "name": "session_start", "event": start, "ctx": untrusted }),
    )
    .await;
    let (id, _) = host.expect_request("settings.get").await;
    host.answer(&id, json!({ "maxDepth": 7 })).await;
    let (id, params) = host.expect_request("ui.notify").await;
    assert_eq!(params["type"], "warning");
    assert!(
        params["message"]
            .as_str()
            .unwrap()
            .contains("because the project is not trusted"),
        "{params}"
    );
    host.answer(&id, Value::Null).await;
    host.expect_response(1).await;

    host.request(
        2,
        "command.run",
        json!({ "name": "context-include:status", "args": "", "ctx": ctx(&root) }),
    )
    .await;
    let (id, params) = host.expect_request("ui.notify").await;
    let message = params["message"].as_str().unwrap();
    assert!(message.contains("maxDepth: 7\n"), "{message}");
    host.answer(&id, Value::Null).await;
    host.expect_response(2).await;
    host.finish().await.unwrap();
}
