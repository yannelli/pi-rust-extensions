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
        json!(["session_start", "before_agent_start", "input", "tool_call"])
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

    let params = json!({ "name": "deferred", "args": "status", "ctx": ctx(&root) });
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

#[tokio::test]
async fn one_off_mcp_tool_is_gone_while_its_sibling_stays() {
    let setting = json!({ "tools": { "mcp__gh__*": "names", "mcp__gh__issue": "off" } });
    let fake = Fake::new(
        setting,
        &["read", "mcp__gh__issue", "mcp__gh__pr", "deferred_search"],
    );
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let start = json!({ "type": "session_start", "reason": "startup" });
    event(&mut host, &fake, 1, "session_start", start, "/work").await;
    let plain = json!({ "type": "before_agent_start", "systemPromptOptions": {} });
    let reply = event(&mut host, &fake, 2, "before_agent_start", plain, "/work").await;
    assert_eq!(fake.active(), ["read", "deferred_search"]);
    let index = reply["patch"]["systemPromptOptions"]["sections"]["deferred"]
        .as_str()
        .unwrap();
    assert!(
        index.contains("Deferred tools: mcp__gh__pr\n")
            || index.ends_with("Deferred tools: mcp__gh__pr"),
        "{index}"
    );
    assert!(!index.contains("issue"), "{index}");

    for (id, query) in [
        (3, "select:mcp__gh__issue"),
        (4, "select:issue"),
        (5, "issue"),
    ] {
        let result = search(&mut host, &fake, id, json!({ "query": query }), "/work").await;
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.starts_with("No deferred item matches"),
            "{query}: {text}"
        );
    }
    assert_eq!(fake.active(), ["read", "deferred_search"]);

    let call = |name: &str| json!({ "type": "tool_call", "toolName": name, "toolCallId": "t", "input": {} });
    let blocked = event(
        &mut host,
        &fake,
        6,
        "tool_call",
        call("mcp__gh__issue"),
        "/work",
    )
    .await;
    assert_eq!(blocked["result"]["block"], true, "{blocked}");
    assert!(
        blocked["result"]["reason"]
            .as_str()
            .unwrap()
            .contains("mcp__gh__issue is turned off"),
        "{blocked}"
    );
    let sibling = event(
        &mut host,
        &fake,
        7,
        "tool_call",
        call("mcp__gh__pr"),
        "/work",
    )
    .await;
    assert_eq!(sibling, json!({}));

    let result = search(
        &mut host,
        &fake,
        8,
        json!({ "query": "select:pr" }),
        "/work",
    )
    .await;
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("Loaded 1 tool(s)")
    );
    assert_eq!(fake.active(), ["read", "deferred_search", "mcp__gh__pr"]);

    let params = json!({ "name": "deferred", "args": "status", "ctx": ctx("/work") });
    host.drive(9, "command.run", params, |m, p| fake.answer(m, p))
        .await;
    let status = fake.calls_of("ui.notify").pop().unwrap();
    let text = status["message"].as_str().unwrap();
    assert!(
        text.contains("tools: 0 names, 0 deferred, 1 off") && text.contains("mcp__gh__issue (off)"),
        "{text}"
    );
}

#[tokio::test]
async fn user_skills_leave_the_prompt_and_off_skills_block_their_command() {
    let setting = json!({ "skills": { "docx": "user", "pdf": "off" } });
    let fake = Fake::new(setting, &["read", "deferred_search"]);
    let mut host = TestHost::start(extension());
    host.initialize().await;
    let start = json!({ "type": "session_start", "reason": "startup" });
    event(&mut host, &fake, 1, "session_start", start, "/w").await;
    let reply = event(
        &mut host,
        &fake,
        2,
        "before_agent_start",
        before("/s"),
        "/w",
    )
    .await;
    let skills = reply["patch"]["systemPromptOptions"]["skills"]
        .as_array()
        .unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(
        (
            skills[0]["name"].as_str(),
            skills[0]["disableModelInvocation"].as_bool()
        ),
        (Some("docx"), Some(true))
    );
    assert!(
        !fake.active().contains(&"deferred_search".to_owned()),
        "nothing is searchable"
    );
    let result = search(&mut host, &fake, 3, json!({ "query": "select:docx" }), "/w").await;
    assert_eq!(result["content"][0]["text"], "Nothing is deferred.");

    let input = |text: &str| json!({ "type": "input", "text": text, "source": "interactive" });
    let user = event(
        &mut host,
        &fake,
        4,
        "input",
        input("/skill:docx make a file"),
        "/w",
    )
    .await;
    assert_eq!(user, json!({}), "a user skill still runs");
    let off = event(&mut host, &fake, 5, "input", input("/skill:pdf now"), "/w").await;
    assert_eq!(off["result"]["action"], "handled");
    let note = fake.calls_of("ui.notify").pop().unwrap();
    assert!(
        note["message"]
            .as_str()
            .unwrap()
            .starts_with("The skill pdf is off"),
        "{note}"
    );
    assert_eq!(
        event(&mut host, &fake, 6, "input", input("hello"), "/w").await,
        json!({})
    );
}
