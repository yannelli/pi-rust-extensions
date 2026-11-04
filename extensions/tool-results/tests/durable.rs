mod common;

use common::{Run, notice_id, text};
use pi_ext_tool_results::store::{Meta, Store, root};
use serde_json::{Value, json};

fn big() -> String {
    (1..=2500)
        .map(|n| format!("row {n} of the build log\n"))
        .collect()
}

async fn store_big(run: &mut Run, call: &str) -> (String, Value) {
    let reply = run
        .result(
            "bash",
            call,
            json!([{ "type": "text", "text": big() }]),
            false,
        )
        .await;
    let id = notice_id(text(&reply));
    (id, reply["patch"]["details"]["toolResults"].clone())
}

#[tokio::test]
async fn results_survive_a_restart_a_resume_and_a_fork() {
    let dir = tempfile::tempdir().unwrap();
    let mut first = Run::start(dir.path(), "s-1", &[]).await;
    let (id, record) = store_big(&mut first, "call_7").await;
    assert_eq!(record["chars"], big().chars().count());
    first
        .event(
            "session_shutdown",
            json!({ "type": "session_shutdown", "reason": "quit" }),
        )
        .await;
    first.host.finish().await.unwrap();

    for session in ["s-1", "s-2-forked"] {
        let mut later = Run::start(dir.path(), session, &[]).await;
        let full = later.get(json!({ "result_id": id, "mode": "full" })).await;
        assert_eq!(
            full["content"][0]["text"].as_str().unwrap(),
            big().trim_end_matches('\n').to_owned() + "\n",
            "{session}"
        );
        let reload = json!({ "type": "session_start", "reason": "reload" });
        later.event("session_start", reload).await;
        let lines = later
            .get(json!({ "result_id": id, "start": 2500, "end": 2500 }))
            .await;
        assert!(
            lines["content"][0]["text"]
                .as_str()
                .unwrap()
                .ends_with("2500\trow 2500 of the build log")
        );
    }
}

#[tokio::test]
async fn a_purged_result_gives_a_clear_error_or_the_copy_in_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut run = Run::start(dir.path(), "s-1", &[]).await;
    let (id, record) = store_big(&mut run, "call_1").await;
    let listed = run.command("list").await;
    assert!(
        listed.contains("1 stored results") && listed.contains(&id),
        "{listed}"
    );
    assert_eq!(
        run.command(&format!("purge {id}")).await,
        "tool-results: removed 1 stored results."
    );
    run.entries = json!([{ "type": "message", "message": { "role": "toolResult", "details": { "exitCode": 0, "toolResults": record } } }]);
    let gone = run.get(json!({ "result_id": id })).await;
    assert_eq!(gone["isError"], true);
    let why = gone["content"][0]["text"].as_str().unwrap();
    assert!(
        why.contains("is no longer stored") && why.contains("sha256:"),
        "{why}"
    );

    std::fs::write(
        dir.path().join("ext/tool-results.json"),
        r#"{ "embedOriginal": true }"#,
    )
    .unwrap();
    run.event(
        "session_start",
        json!({ "type": "session_start", "reason": "reload" }),
    )
    .await;
    let (id, record) = store_big(&mut run, "call_2").await;
    assert_eq!(record["original"].as_str().unwrap().len(), big().len());
    run.command("purge session").await;
    assert!(Store::new(root(dir.path())).list().is_empty());
    run.entries = json!([{ "type": "message", "message": { "role": "toolResult", "details": { "toolResults": record } } }]);
    let full = run.get(json!({ "result_id": id, "mode": "full" })).await;
    assert_eq!(
        full["content"][0]["text"].as_str().unwrap().len(),
        big().len()
    );
}

#[tokio::test]
async fn the_expander_follows_the_config_and_draws_the_stored_text() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("ext")).unwrap();
    std::fs::write(
        dir.path().join("ext/tool-results.json"),
        r#"{ "expandedMaxHeight": 12 }"#,
    )
    .unwrap();
    let mut run = Run::start(dir.path(), "s-1", &["toolResultExpander"]).await;
    let sent: Vec<&Value> = run
        .sent
        .iter()
        .filter(|(m, _)| m == "register.toolResultExpander")
        .map(|(_, p)| p)
        .collect();
    assert_eq!(
        sent,
        [&json!({ "detailsKey": "toolResults", "maxHeight": 12, "enabled": true })]
    );
    let old = Run::start(dir.path(), "s-1", &[]).await;
    assert!(
        !old.sent
            .iter()
            .any(|(m, _)| m == "register.toolResultExpander"),
        "old hosts get no request"
    );
    old.host.finish().await.unwrap();

    let (id, record) = store_big(&mut run, "call_3").await;
    let params = json!({
        "toolName": "bash", "toolCallId": "call_3", "width": 40,
        "result": { "content": [], "details": { "toolResults": record } },
        "options": { "expanded": true, "maxHeight": 12 }
    });
    let lines = run.call("render.toolResultExpanded", params.clone()).await["lines"].clone();
    let lines: Vec<String> = serde_json::from_value(lines).unwrap();
    assert!(
        pi_ext_sdk::style::strip(&lines[0]).starts_with(&format!("Full result {id} · ")),
        "{lines:?}"
    );
    assert_eq!(lines[1], "row 1 of the build log");
    assert_eq!(lines.len(), 2 + 2500);
    run.command(&format!("purge {id}")).await;
    let gone = run.call("render.toolResultExpanded", params).await["lines"][0].clone();
    assert!(
        gone.as_str().unwrap().contains("is no longer stored"),
        "{gone}"
    );
}

#[tokio::test]
async fn retention_runs_at_startup() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(root(dir.path()));
    let old = Meta {
        id: "tr-old".into(),
        created: 1,
        ..Meta::default()
    };
    store.save(&old, "from long ago").unwrap();
    let fresh = Meta {
        id: "tr-fresh".into(),
        created: pi_ext_tool_results::store::now_secs(),
        ..Meta::default()
    };
    store.save(&fresh, "from today").unwrap();
    let _run = Run::start(dir.path(), "s-1", &[]).await;
    for _ in 0..100 {
        if store.load("tr-old").is_none() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(store.load("tr-old"), None);
    assert_eq!(store.load("tr-fresh").as_deref(), Some("from today"));
}
