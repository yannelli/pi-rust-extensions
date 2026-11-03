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

#[tokio::test]
async fn repo_rules_follow_the_origin_remote_and_set_writes_them() {
    let dir = tempfile::tempdir().unwrap();
    let (agent, work) = (dir.path().join("agent"), dir.path().join("work"));
    std::fs::create_dir_all(&agent).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&work)
            .args(args)
            .output()
            .is_ok_and(|o| o.status.success())
    };
    if !git(&["init", "-q"]) {
        eprintln!("skipping: git is not available");
        return;
    }
    assert!(git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/Yannelli/Pi-Rust.git"
    ]));
    let setting = json!({ "tools": { "bash": "names" }, "repos": { "yannelli/pi-rust": { "tools": { "bash": "off" } } } });
    std::fs::write(
        agent.join("settings.json"),
        json!({ "deferred": setting }).to_string(),
    )
    .unwrap();
    let fake = Fake::new(setting, &["read", "bash"]);
    let mut host = TestHost::start(extension()).agent_dir(agent.display().to_string());
    host.initialize().await;
    let cwd = work.display().to_string();
    let start = json!({ "type": "session_start", "reason": "startup" });
    let params = json!({ "name": "session_start", "event": start, "ctx": ctx(&cwd) });
    host.drive(1, "event", params, |m, p| fake.answer(m, p))
        .await;
    let plain = json!({ "type": "before_agent_start", "systemPromptOptions": {} });
    let params = json!({ "name": "before_agent_start", "event": plain, "ctx": ctx(&cwd) });
    host.drive(2, "event", params, |m, p| fake.answer(m, p))
        .await;
    assert_eq!(fake.active(), ["read"]);

    let params = json!({ "name": "deferred", "args": "status", "ctx": ctx(&cwd) });
    host.drive(3, "command.run", params, |m, p| fake.answer(m, p))
        .await;
    let text = fake.calls_of("ui.notify").pop().unwrap()["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(text.contains("repo: Yannelli/Pi-Rust (origin)"), "{text}");
    assert!(
        text.contains("tools bash = off [repo yannelli/pi-rust]"),
        "{text}"
    );

    let params =
        json!({ "name": "deferred", "args": "set skills deploy user --repo", "ctx": ctx(&cwd) });
    host.drive(4, "command.run", params, |m, p| fake.answer(m, p))
        .await;
    let set = fake.calls_of("settings.set").pop().unwrap();
    assert_eq!(
        set["value"]["repos"],
        json!({ "yannelli/pi-rust": { "tools": { "bash": "off" }, "skills": { "deploy": "user" } } })
    );
    assert_eq!(set["scope"], "global");
    let note = fake.calls_of("ui.notify").pop().unwrap();
    assert!(
        note["message"]
            .as_str()
            .unwrap()
            .starts_with("Set skills `deploy` to user for repo Yannelli/Pi-Rust"),
        "{note}"
    );

    for (id, args) in [
        (5, "set tools bash user"),
        (6, "set tools bash off --repo a/b --project"),
    ] {
        let params = json!({ "name": "deferred", "args": args, "ctx": ctx(&cwd) });
        host.drive(id, "command.run", params, |m, p| fake.answer(m, p))
            .await;
        assert_eq!(
            fake.calls_of("ui.notify").pop().unwrap()["type"],
            "error",
            "{args}"
        );
    }
    let params = json!({ "name": "deferred", "args": "unset tools bash --repo other/repo", "ctx": ctx(&cwd) });
    host.drive(7, "command.run", params, |m, p| fake.answer(m, p))
        .await;
    let set = fake.calls_of("settings.set").pop().unwrap();
    assert_eq!(
        set["value"]["repos"]["yannelli/pi-rust"]["tools"],
        json!({ "bash": "off" })
    );
    assert!(set["value"]["repos"].get("other/repo").is_none());
}
