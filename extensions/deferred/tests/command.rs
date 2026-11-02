mod common;

use common::{Fake, ctx};
use pi_ext_deferred::extension;
use pi_ext_sdk::test_util::TestHost;
use serde_json::json;

#[tokio::test]
async fn set_command_writes_the_rule_into_settings() {
    let dir = tempfile::tempdir().unwrap();
    let agent = dir.path().display().to_string();
    std::fs::write(
        format!("{agent}/settings.json"),
        r#"{"deferred": {"tools": {"a": "off"}}}"#,
    )
    .unwrap();
    let fake = Fake::new(json!({}), &[]);
    let mut host = TestHost::start(extension()).agent_dir(agent.clone());
    host.initialize().await;
    let params =
        json!({ "name": "deferred", "args": "set skills pdf name-only", "ctx": ctx("/work") });
    host.drive(1, "command.run", params, |m, p| fake.answer(m, p))
        .await;
    let set = fake.calls_of("settings.set");
    assert_eq!(
        set,
        [
            json!({ "namespace": "deferred", "value": { "tools": { "a": "off" }, "skills": { "pdf": "names" } }, "scope": "global" })
        ]
    );
    let note = fake.calls_of("ui.notify").pop().unwrap();
    assert!(
        note["message"]
            .as_str()
            .unwrap()
            .starts_with("Set skills `pdf` to names"),
        "{note}"
    );

    let params = json!({ "name": "deferred", "args": "set tools", "ctx": ctx("/work") });
    host.drive(2, "command.run", params, |m, p| fake.answer(m, p))
        .await;
    let note = fake.calls_of("ui.notify").pop().unwrap();
    assert_eq!(note["type"], "error");
}
