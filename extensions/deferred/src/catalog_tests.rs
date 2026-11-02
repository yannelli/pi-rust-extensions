use super::*;
use serde_json::json;

fn tool(name: &str, path: &str, source: &str) -> Value {
    json!({
        "name": name, "description": format!("{name} does things"), "exposure": "direct",
        "parameters": { "type": "object" }, "sourceInfo": { "path": path, "source": source }
    })
}

fn tools() -> Vec<Value> {
    vec![
        tool("read", "builtin:read", "builtin"),
        tool("bash", "builtin:bash", "builtin"),
        tool("mcp__gh__issue", "builtin:mcp", "builtin"),
        tool("mcp__gh__pr", "builtin:mcp", "builtin"),
        tool("web_fetch", "/ext/web", "extension"),
        tool(TOOL, "/ext/deferred", "extension"),
    ]
}

fn names(list: &[String]) -> Vec<&str> {
    list.iter().map(String::as_str).collect()
}

fn run(config: Value, active: &[&str], options: Value, window: Option<u64>) -> Plan {
    run_with(
        config,
        active,
        options,
        window,
        &BTreeSet::new(),
        &BTreeSet::new(),
    )
}

fn run_with(
    config: Value,
    active: &[&str],
    options: Value,
    window: Option<u64>,
    loaded: &BTreeSet<String>,
    removed: &BTreeSet<String>,
) -> Plan {
    let config = Config::merge([("test", Some(&config))]);
    let active: Vec<String> = active.iter().map(|s| (*s).to_owned()).collect();
    let tools = tools();
    plan(&Inputs {
        config: &config,
        tools: &tools,
        active: &active,
        options: &options,
        context_window: window,
        loaded,
        removed,
    })
}

#[test]
fn no_rules_keep_everything_and_drop_the_search_tool() {
    let plan = run(json!({}), &["read", "bash", TOOL], json!({}), None);
    assert!(plan.items.is_empty());
    assert_eq!(names(&plan.active), ["read", "bash"]);
    assert_eq!(
        plan,
        Plan {
            active: plan.active.clone(),
            ..Plan::default()
        }
    );
}

#[test]
fn rules_defer_disable_and_add_the_search_tool() {
    let config = json!({ "tools": { "mcp__*": "names", "web_fetch": "off", "read": "deferred" } });
    let active = ["read", "bash", "mcp__gh__issue", "web_fetch", TOOL];
    let plan = run(config, &active, json!({}), None);
    assert_eq!(names(&plan.active), ["bash", TOOL]);
    assert_eq!(
        names(&plan.newly_removed),
        ["read", "mcp__gh__issue", "web_fetch"]
    );
    let items: Vec<(&str, Mode)> = plan
        .items
        .iter()
        .map(|i| (i.name.as_str(), i.mode))
        .collect();
    assert_eq!(
        items,
        [
            ("read", Mode::Deferred),
            ("mcp__gh__issue", Mode::Names),
            ("mcp__gh__pr", Mode::Names)
        ]
    );
    assert_eq!(plan.off.len(), 1);
}

#[test]
fn loaded_removed_and_restored_tools() {
    let config = json!({ "tools": { "mcp__*": "names", "bash": "on" } });
    let loaded = BTreeSet::from(["mcp__gh__pr".to_owned()]);
    let removed = BTreeSet::from(["mcp__gh__issue".to_owned(), "bash".to_owned()]);
    let plan = run_with(
        config,
        &["read", "mcp__gh__issue"],
        json!({}),
        None,
        &loaded,
        &removed,
    );
    assert_eq!(names(&plan.adopted), ["mcp__gh__issue"]);
    assert_eq!(names(&plan.restored), ["bash"]);
    assert_eq!(
        names(&plan.active),
        ["read", "mcp__gh__issue", "bash", "mcp__gh__pr"]
    );
    assert!(plan.items.is_empty());
}

#[test]
fn auto_defers_addon_tools_past_the_threshold() {
    let cost: usize = tools()[2..5].iter().map(tool_tokens).sum();
    let window = (cost * 100 / 10) as u64;
    let plan = run(
        json!({ "tools": { "web_fetch": "on" } }),
        &["read"],
        json!({}),
        Some(window - 1),
    );
    assert!(!plan.auto, "explicit rules do not count toward auto");
    let plan = run(json!({}), &["read"], json!({}), Some(window - 1));
    assert!(plan.auto);
    let auto: Vec<&str> = plan.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(auto, ["mcp__gh__issue", "mcp__gh__pr", "web_fetch"]);
    assert!(!run(json!({}), &["read"], json!({}), Some(window + 10)).auto);
    assert!(!run(json!({ "auto": false }), &["read"], json!({}), Some(10)).auto);
    assert!(!run(json!({}), &["read"], json!({}), None).auto);
}

#[test]
fn skills_context_and_sections() {
    let skill = |name: &str, hidden: bool| json!({ "name": name, "description": "d", "filePath": format!("/s/{name}/SKILL.md"), "baseDir": format!("/s/{name}"), "disableModelInvocation": hidden });
    let options = json!({
        "skills": [skill("pdf", false), skill("docx", false), skill("old", false), skill("manual", true)],
        "contextFiles": [{ "path": "/w/AGENTS.md", "content": "# Rules\nbody" }, { "path": "/w/docs/AGENTS.md", "content": "x" }],
        "sections": { "mcp_servers": "server list", "other": "keep", "deferred": "ours" }
    });
    let config = json!({
        "skills": { "*": "names", "pdf": "on", "old": "off", "manual": "deferred" },
        "context": { "docs/AGENTS.md": "deferred" },
        "sections": { "mcp_servers": "names", "deferred": "off" }
    });
    let plan = run(config, &[TOOL], options, None);
    let kept: Vec<&str> = plan
        .skills
        .as_ref()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(kept, ["pdf", "manual"]);
    let files = plan.context_files.as_ref().unwrap();
    assert_eq!(
        (files.len(), files[0]["path"].as_str()),
        (1, Some("/w/AGENTS.md"))
    );
    assert_eq!(names(&plan.drop_sections), ["mcp_servers"]);
    let items: Vec<(Kind, &str)> = plan
        .items
        .iter()
        .map(|i| (i.kind, i.name.as_str()))
        .collect();
    assert_eq!(
        items,
        [
            (Kind::Skill, "docx"),
            (Kind::Context, "/w/docs/AGENTS.md"),
            (Kind::Section, "mcp_servers")
        ]
    );
    assert_eq!(
        plan.items[0].body,
        Body::Skill {
            file: "/s/docx/SKILL.md".into(),
            base_dir: "/s/docx".into()
        }
    );
    assert_eq!(plan.off, [(Kind::Skill, "old".to_owned(), plan.off[0].2)]);
    assert_eq!(names(&plan.active), [TOOL]);
}

#[test]
fn unchanged_lists_are_not_patched() {
    let options = json!({ "skills": [{ "name": "a", "filePath": "/a" }], "contextFiles": [{ "path": "/w/AGENTS.md", "content": "" }] });
    let plan = run(json!({ "skills": { "b": "off" } }), &[], options, None);
    assert_eq!((plan.skills, plan.context_files), (None, None));
}

#[test]
fn summary_line_skips_blanks_and_clips() {
    assert_eq!(summary_line("\n\n## Title  \nmore"), "Title");
    let long = "x".repeat(130);
    assert_eq!(summary_line(&long).chars().count(), 121);
    assert_eq!(summary_line(""), "");
}
