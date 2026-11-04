mod common;

use std::collections::BTreeSet;
use std::path::Path;

use common::{TestHost, ctx, dirs, scripted_events};
use pi_ext_subagents::agent::{Exposure, Source};
use pi_ext_subagents::context::{self, AGENT_PARAM, KIND};
use pi_ext_subagents::defaults::builtin_agents;
use pi_ext_subagents::registry::merge_file;
use serde_json::{Value, json};

const REVIEWER: &str =
    "---\nname: reviewer\ndescription: Reviews diffs.\nexposure: Deferred\n---\nReview.";

#[test]
fn exposure_frontmatter_is_parsed_and_layered() {
    let mut agents = Vec::new();
    merge_file(
        &mut agents,
        Path::new("/g/reviewer.md"),
        REVIEWER,
        Source::Global,
    );
    assert_eq!(agents[0].exposure, Some(Exposure::Deferred));
    let bad = "---\nname: reviewer\nexposure: hidden\n---\n";
    merge_file(
        &mut agents,
        Path::new("/p/reviewer.md"),
        bad,
        Source::Project,
    );
    assert_eq!(
        agents[0].exposure,
        Some(Exposure::Deferred),
        "an invalid value is ignored and the lower layer stays"
    );
    let direct = "---\nname: reviewer\nexposure: direct\n---\n";
    merge_file(&mut agents, Path::new("/p/r2.md"), direct, Source::Project);
    assert_eq!(agents[0].exposure, Some(Exposure::Direct));
    assert_eq!(builtin_agents()[0].exposure, None);
}

#[test]
fn entries_use_the_tool_name_param_path_and_exposure() {
    let spec = pi_ext_subagents::tool::spec();
    assert!(spec.parameters["properties"][AGENT_PARAM].is_object());
    let explore = builtin_agents().remove(0);
    let entry = context::entry(&explore, &spec.name).unwrap();
    assert_eq!(entry["kind"], KIND);
    assert_eq!(entry["name"], "explore");
    assert_eq!(
        entry["usage"],
        "Call the subagent tool with agent \"explore\"."
    );
    assert!(entry.get("path").is_none() && entry.get("exposure").is_none());
    let mut agents = Vec::new();
    merge_file(
        &mut agents,
        Path::new("/g/reviewer.md"),
        REVIEWER,
        Source::Global,
    );
    let entry = context::entry(&agents[0], &spec.name).unwrap();
    assert_eq!(entry["description"], "Reviews diffs.");
    assert_eq!(entry["path"], "/g/reviewer.md");
    assert_eq!(entry["exposure"], "deferred");
    agents[0].description = " ".into();
    assert!(context::entry(&agents[0], &spec.name).is_none());
}

#[test]
fn section_lists_only_agents_pi_does_not_list() {
    let agents = builtin_agents();
    let full = context::prompt_section(&agents, None);
    assert!(full.contains("shows their tools and models):\n- `explore`"), "{full}");
    let offered: BTreeSet<String> = ["explore", "build"].map(String::from).into();
    let section = context::prompt_section(&agents, Some(&offered));
    assert!(section.contains("<available_context>"), "{section}");
    assert!(section.contains("Other agents:\n- `advisor`"), "{section}");
    assert!(!section.contains("`explore`") && !section.contains("`build`"));
    let all: BTreeSet<String> = agents.iter().map(|a| a.name.clone()).collect();
    assert!(!context::prompt_section(&agents, Some(&all)).contains("Other agents"));
}

async fn event(host: &mut TestHost, id: u64, name: &str, event: Value, cwd: &Path) -> Value {
    let params = json!({ "name": name, "event": event, "ctx": ctx(cwd) });
    host.request(id, "event", params).await;
    host.until_response(id).await["result"].clone()
}

async fn discover(host: &mut TestHost, id: u64, cwd: &Path) -> Value {
    let ev = json!({ "type": "resources_discover", "cwd": cwd, "reason": "startup" });
    event(host, id, "resources_discover", ev, cwd).await["result"].clone()
}

async fn section(host: &mut TestHost, id: u64, cwd: &Path) -> String {
    let ev = json!({ "type": "before_agent_start", "prompt": "hi",
        "systemPromptOptions": { "sections": {}, "selectedTools": ["subagent"] } });
    let result = event(host, id, "before_agent_start", ev, cwd).await;
    let text = &result["patch"]["systemPromptOptions"]["sections"]["subagents"];
    text.as_str().unwrap().to_owned()
}

fn write_agent(dir: &Path, file: &str, text: &str) {
    let agents = dir.join("ext/subagents/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join(file), text).unwrap();
}

#[tokio::test]
async fn new_host_gets_entries_and_the_section_skips_them() {
    let d = dirs(&scripted_events("x"), 0);
    write_agent(&d.agent, "reviewer.md", REVIEWER);
    let mut host = TestHost::start(pi_ext_subagents::extension());
    let init = host
        .initialize_with(&d.agent, &d.cwd, &["contextEntries"])
        .await;
    let events = init["events"].as_array().unwrap();
    assert!(events.iter().any(|e| e == "resources_discover"));
    let found = discover(&mut host, 1, &d.cwd).await;
    let entries = found["contextEntries"].as_array().unwrap();
    let names: Vec<&str> = entries
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["explore", "build", "advisor", "reviewer"]);
    let reviewer = &entries[3];
    assert_eq!(reviewer["exposure"], "deferred");
    let path = d.agent.join("ext/subagents/agents/reviewer.md");
    assert_eq!(reviewer["path"], path.display().to_string());

    let text = section(&mut host, 2, &d.cwd).await;
    assert!(
        !text.contains("`explore`") && !text.contains("`reviewer`"),
        "{text}"
    );
    assert!(!text.contains("Other agents"), "{text}");

    // An agent made after the discovery shows in the section until a `/reload`.
    write_agent(
        &d.agent,
        "linter.md",
        "---\nname: linter\ndescription: Lints.\n---\nLint.",
    );
    let text = section(&mut host, 3, &d.cwd).await;
    assert!(text.contains("Other agents:\n- `linter`: Lints."), "{text}");
    assert!(!text.contains("`reviewer`"), "{text}");
    host.finish().await;
}

#[tokio::test]
async fn old_host_keeps_the_full_section() {
    let d = dirs(&scripted_events("x"), 0);
    write_agent(&d.agent, "reviewer.md", REVIEWER);
    let mut host = TestHost::start(pi_ext_subagents::extension());
    host.initialize(&d.agent, &d.cwd).await;
    let found = discover(&mut host, 1, &d.cwd).await;
    assert!(found.is_null(), "{found}");
    let text = section(&mut host, 2, &d.cwd).await;
    assert!(text.contains("shows their tools and models):\n- `explore`"), "{text}");
    assert!(text.contains("- `reviewer`: Reviews diffs."), "{text}");
    host.finish().await;
}

#[tokio::test]
async fn advertise_false_offers_no_entries() {
    let d = dirs(&scripted_events("x"), 0);
    let config = json!({ "advertise": false });
    std::fs::write(d.agent.join("ext/subagents.json"), config.to_string()).unwrap();
    let mut host = TestHost::start(pi_ext_subagents::extension());
    host.initialize_with(&d.agent, &d.cwd, &["contextEntries"])
        .await;
    let found = discover(&mut host, 1, &d.cwd).await;
    assert!(found.is_null(), "{found}");
    host.finish().await;
}
