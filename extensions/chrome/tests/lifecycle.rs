mod common;

use common::Pi;
use common::fake_cdp::FakeCdp;
use pi_ext_chrome::extension;
use serde_json::json;

#[tokio::test]
async fn untrusted_project_config_is_ignored() {
    let fake = FakeCdp::start().await;
    let (agent, cwd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::create_dir_all(agent.path().join("ext")).unwrap();
    let global = json!({ "mode": "connect", "port": fake.http_port });
    std::fs::write(agent.path().join("ext/chrome.json"), global.to_string()).unwrap();
    std::fs::create_dir_all(cwd.path().join(".pi/ext")).unwrap();
    let project = json!({ "mode": "launch", "port": 1, "chromePath": "/bin/false" });
    std::fs::write(cwd.path().join(".pi/ext/chrome.json"), project.to_string()).unwrap();

    let mut pi = Pi::start(extension(), cwd.path());
    pi.initialize(agent.path()).await;
    pi.event("session_start", json!({ "reason": "startup" }))
        .await;
    pi.command("doctor").await;
    let notes: Vec<String> = pi
        .seen
        .iter()
        .filter(|m| m["method"] == "ui.notify")
        .map(|m| m["params"]["message"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("because the project is not trusted")),
        "{notes:?}"
    );
    let doctor = notes.last().unwrap();
    assert!(
        doctor.contains("(ignored, the project is not trusted)"),
        "{doctor}"
    );
    assert!(
        doctor.contains(&format!(
            "Mode: connect, endpoint 127.0.0.1:{}",
            fake.http_port
        )),
        "{doctor}"
    );
    assert!(
        doctor.contains("DevTools: reachable, FakeChrome/1.0"),
        "{doctor}"
    );
    assert!(doctor.contains("Authorization: locked"), "{doctor}");
    pi.finish().await;
}

#[tokio::test]
async fn session_switch_resets_grant_and_tabs() {
    let fake = FakeCdp::start().await;
    let (agent, cwd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::create_dir_all(agent.path().join("ext")).unwrap();
    let global = json!({ "mode": "connect", "port": fake.http_port });
    std::fs::write(agent.path().join("ext/chrome.json"), global.to_string()).unwrap();
    let mut pi = Pi::start(extension(), cwd.path());
    pi.initialize(agent.path()).await;
    pi.event("session_start", json!({ "reason": "startup" }))
        .await;
    pi.command("authorize 30m").await;
    pi.command("background off").await;
    pi.tool(
        "chrome_tab",
        json!({ "action": "new", "url": "https://a.dev" }),
    )
    .await;
    assert_eq!(fake.calls_of("Target.activateTarget").len(), 1);

    pi.session_id = "s2".to_owned();
    pi.event("session_start", json!({ "reason": "new" })).await;
    assert_eq!(pi.chrome_active(), 0);
    assert_eq!(
        fake.calls_of("Target.closeTarget")[0]["params"]["targetId"],
        "T1"
    );
    let result = pi.tool("chrome_tab", json!({ "action": "list" })).await;
    assert_eq!(result["isError"], true);

    pi.command("authorize 30m").await;
    pi.tool("chrome_tab", json!({ "action": "new" })).await;
    assert_eq!(
        fake.calls_of("Target.activateTarget").len(),
        1,
        "background resets to on"
    );
    let entry = &pi.branch.last().unwrap()["data"];
    assert_eq!(
        (entry["action"].as_str(), entry["sessionId"].as_str()),
        (Some("authorized"), Some("s2"))
    );
    pi.finish().await;
}
