mod common;

use common::{Run, notice_id, text};
use pi_ext_tool_results::lines::short;
use serde_json::json;

#[tokio::test]
async fn cleans_results_and_leaves_others() {
    let dir = tempfile::tempdir().unwrap();
    let mut run = Run::start(dir.path(), "s-1", &[]).await;
    let noisy = json!([{ "type": "text", "text": "\u{1b}[31merror\u{1b}[0m   \n\n\n\nnext" }]);
    let reply = run.result("bash", "c1", noisy.clone(), false).await;
    assert_eq!(text(&reply), "error\n\nnext");
    let record = &reply["patch"]["details"]["toolResults"];
    assert_eq!(record["chars"], 16, "the full text after the cleanup steps");
    assert!(record["hash"].as_str().unwrap().starts_with("sha256:"));
    assert!(
        record.get("original").is_none(),
        "embedOriginal is off by default"
    );
    let reply = run.result("bash", "c2", noisy.clone(), true).await;
    assert_eq!(text(&reply), "error\n\nnext", "errors get the cleanup");
    assert_eq!(
        run.result("tool_result_get", "c3", noisy, false).await,
        json!({})
    );
    let image = json!([{ "type": "image", "data": "AAAA", "mimeType": "image/png" }]);
    assert_eq!(run.result("bash", "c4", image, false).await, json!({}));
    let json_text = serde_json::to_string_pretty(
        &json!({ "items": [{ "id": 1, "n": "a" }, { "id": 2, "n": "b" }] }),
    )
    .unwrap();
    let reply = run
        .result(
            "mcp__x__list",
            "c5",
            json!([{ "type": "text", "text": json_text }]),
            false,
        )
        .await;
    assert_eq!(text(&reply), "items[2]{id,n}:\n  1,a\n  2,b");
}

#[tokio::test]
async fn stores_large_results_and_reads_them_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut run = Run::start(dir.path(), "s-1", &[]).await;
    let big: String = (1..=3000)
        .map(|n| format!("entry {n} status=ok\n"))
        .collect();
    let reply = run
        .result(
            "bash",
            "call-big",
            json!([{ "type": "text", "text": big }]),
            false,
        )
        .await;
    let shown = text(&reply);
    assert!(shown.starts_with("entry 1 status=ok\n"), "{shown}");
    let id = notice_id(shown);
    assert_eq!(
        reply["patch"]["details"]["toolResults"]["resultId"],
        id.as_str()
    );
    let notice = format!(
        "Result truncated ({} characters, 3,000 lines).",
        short(big.chars().count())
    );
    assert!(shown.contains(&notice), "{shown}");
    assert!(
        dir.path()
            .join(format!("ext/tool-results/store/{id}.txt"))
            .is_file()
    );

    let lines = run
        .get(json!({ "result_id": id, "mode": "lines", "start": 1500, "end": 1501 }))
        .await;
    let expected = format!(
        "Result {id}: lines 1500-1501 of 3,000\n1500\tentry 1500 status=ok\n1501\tentry 1501 status=ok"
    );
    assert_eq!(lines["content"][0]["text"], expected);
    let found = run.get(json!({ "result_id": id, "mode": "search", "pattern": "entry 2999 ", "context_chars": 5 })).await;
    let found = found["content"][0]["text"].as_str().unwrap();
    assert!(
        found.contains("--- line 2999\n…s=ok\nentry 2999 statu…"),
        "{found}"
    );
    let full = run.get(json!({ "result_id": id, "mode": "full" })).await;
    assert_eq!(
        full["content"][0]["text"].as_str().unwrap().len(),
        big.len()
    );
    let missing = run.get(json!({ "result_id": "tr-none" })).await;
    assert_eq!(missing["isError"], true);
    let bad = run.get(json!({ "result_id": id, "mode": "search" })).await;
    assert_eq!(bad["isError"], true);

    let stop = json!({ "type": "session_shutdown", "reason": "quit" });
    run.event("session_shutdown", stop).await;
    assert!(
        dir.path()
            .join(format!("ext/tool-results/store/{id}.txt"))
            .is_file(),
        "results outlive the session"
    );
    let stats = std::fs::read_to_string(dir.path().join("ext/tool-results/stats.json")).unwrap();
    assert!(stats.contains("\"truncated\": 1"), "{stats}");
}

#[tokio::test]
async fn rules_and_dry_run_from_the_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let config = json!({ "dryRun": false, "tools": { "bash": { "enabled": false }, "mcp__*": { "json": "compact" } } });
    std::fs::create_dir_all(dir.path().join("ext")).unwrap();
    std::fs::write(dir.path().join("ext/tool-results.json"), config.to_string()).unwrap();
    let mut run = Run::start(dir.path(), "s-1", &[]).await;
    let noisy = json!([{ "type": "text", "text": "a   \n\n\n\nb" }]);
    assert_eq!(
        run.result("bash", "c1", noisy.clone(), false).await,
        json!({})
    );
    let pretty = json!([{ "type": "text", "text": "{\n  \"a\": [1, 2]\n}" }]);
    assert_eq!(
        text(&run.result("mcp__s__t", "c2", pretty, false).await),
        "{\"a\":[1,2]}"
    );

    std::fs::write(
        dir.path().join("ext/tool-results.json"),
        json!({ "dryRun": true }).to_string(),
    )
    .unwrap();
    let start = json!({ "type": "session_start", "reason": "reload" });
    run.event("session_start", start).await;
    assert_eq!(run.result("grep", "c3", noisy, false).await, json!({}));
    let stats = run.command("stats").await;
    assert!(stats.contains("mcp__s__t: "), "{stats}");
    assert!(stats.contains("dry run would save: "), "{stats}");
    let status = run.command("").await;
    assert!(status.contains("tool-results (dry run"), "{status}");
}
