mod common;

use common::{Fake, before, ctx, event, search};
use pi_ext_deferred::extension;
use pi_ext_sdk::test_util::TestHost;
use serde_json::{Value, json};

#[tokio::test]
async fn defers_then_loads_tools_skills_and_context() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().display().to_string();
    std::fs::create_dir_all(format!("{root}/docx")).unwrap();
    std::fs::write(
        format!("{root}/docx/SKILL.md"),
        "---\nname: docx\n---\nMake Word files.",
    )
    .unwrap();
    let setting = json!({ "tools": { "mcp__*": "names" }, "skills": { "docx": "deferred" },
                          "context": { "AGENTS.md": "names" }, "sections": { "mcp_servers": "off" } });
    let fake = Fake::new(
        setting,
        &["read", "bash", "mcp__gh__issue", "deferred_search"],
    );
    let mut host = TestHost::start(extension());
    let init = host.initialize().await;
    assert_eq!(
        init["events"],
        json!(["session_start", "before_agent_start"])
    );
    assert_eq!(init["tools"][0]["name"], "deferred_search");

    let start = json!({ "type": "session_start", "reason": "startup" });
    assert_eq!(
        event(&mut host, &fake, 1, "session_start", start, &root).await,
        json!({})
    );

    let reply = event(
        &mut host,
        &fake,
        2,
        "before_agent_start",
        before(&root),
        &root,
    )
    .await;
    assert_eq!(fake.active(), ["read", "bash", "deferred_search"]);
    let options = &reply["patch"]["systemPromptOptions"];
    let index = options["sections"]["deferred"].as_str().unwrap();
    assert!(
        index.contains("Deferred tools: mcp__gh__{issue, pr}"),
        "{index}"
    );
    assert!(
        index.contains(&format!("Deferred context files: {root}/AGENTS.md")),
        "{index}"
    );
    assert!(index.contains("1 more deferred items"), "{index}");
    assert_eq!(options["sections"]["mcp_servers"], Value::Null);
    assert!(
        options["sections"]
            .as_object()
            .unwrap()
            .contains_key("mcp_servers")
    );
    assert_eq!(options["skills"].as_array().unwrap().len(), 1);
    assert_eq!(options["contextFiles"], json!([]));

    let result = search(
        &mut host,
        &fake,
        3,
        json!({ "query": "select:issue" }),
        &root,
    )
    .await;
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("Loaded 1 tool(s)")
            && text.contains("- mcp__gh__issue: mcp__gh__issue tool"),
        "{text}"
    );
    assert_eq!(
        fake.active(),
        ["read", "bash", "deferred_search", "mcp__gh__issue"]
    );
    let entries = fake.calls_of("session.appendEntry");
    assert_eq!(
        entries,
        [json!({ "customType": "deferred-load", "data": { "tools": ["mcp__gh__issue"] } })]
    );

    let result = search(
        &mut host,
        &fake,
        4,
        json!({ "query": "docx", "kind": "skill" }),
        &root,
    )
    .await;
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("<skill name=\"docx\"")
            && text.contains("Make Word files.")
            && !text.contains("name: docx"),
        "{text}"
    );

    let result = search(&mut host, &fake, 5, json!({ "query": "+house" }), &root).await;
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Tabs.")
    );
    let result = search(
        &mut host,
        &fake,
        6,
        json!({ "query": "select:mcp__gh__issue" }),
        &root,
    )
    .await;
    assert_eq!(
        result["content"][0]["text"],
        "Already loaded: mcp__gh__issue."
    );
    let result = search(
        &mut host,
        &fake,
        7,
        json!({ "query": "x", "kind": "bogus" }),
        &root,
    )
    .await;
    assert_eq!(result["isError"], true);

    let reply = event(
        &mut host,
        &fake,
        8,
        "before_agent_start",
        before(&root),
        &root,
    )
    .await;
    assert_eq!(
        fake.active(),
        ["read", "bash", "deferred_search", "mcp__gh__issue"]
    );
    let index = reply["patch"]["systemPromptOptions"]["sections"]["deferred"]
        .as_str()
        .unwrap();
    assert!(index.contains("Deferred tools: mcp__gh__pr\n"), "{index}");

    let params = json!({ "name": "deferred", "args": "", "ctx": ctx(&root) });
    host.drive(9, "command.run", params, |m, p| fake.answer(m, p))
        .await;
    let status = fake.calls_of("ui.notify").pop().unwrap();
    let text = status["message"].as_str().unwrap();
    assert!(
        text.contains("tools: 1 names")
            && text.contains("loaded tools this session: mcp__gh__issue"),
        "{text}"
    );
}

#[tokio::test]
async fn resume_restores_loaded_tools_and_nothing_deferred_hides_the_tool() {
    let mut fake = Fake::new(
        json!({ "tools": { "mcp__*": "deferred" } }),
        &["read", "deferred_search"],
    );
    fake.branch = json!([{ "type": "custom", "customType": "deferred-load", "data": { "tools": ["mcp__gh__pr"] } }]);
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let start = json!({ "type": "session_start", "reason": "resume" });
    event(&mut host, &fake, 1, "session_start", start, "/work").await;
    let plain = json!({ "type": "before_agent_start", "systemPromptOptions": {} });
    event(
        &mut host,
        &fake,
        2,
        "before_agent_start",
        plain.clone(),
        "/work",
    )
    .await;
    assert_eq!(fake.active(), ["read", "deferred_search", "mcp__gh__pr"]);

    let mut fake = Fake::new(json!({}), &["read", "deferred_search"]);
    fake.setting = json!({ "auto": false });
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let reply = event(&mut host, &fake, 1, "before_agent_start", plain, "/work").await;
    assert_eq!(fake.active(), ["read"]);
    assert_eq!(
        reply["patch"]["systemPromptOptions"]["sections"]["deferred"],
        Value::Null
    );
}
