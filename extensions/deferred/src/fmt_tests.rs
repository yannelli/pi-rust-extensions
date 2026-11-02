use super::*;
use crate::catalog::Plan;
use serde_json::json;

fn item(kind: Kind, name: &str, mode: Mode, body: Body) -> Item {
    Item {
        kind,
        name: name.into(),
        description: "desc".into(),
        mode,
        tokens: 10,
        body,
    }
}

#[test]
fn index_lists_names_and_counts_hidden_items() {
    assert_eq!(index(&[]), None);
    let items = vec![
        item(Kind::Tool, "mcp__gh__issue", Mode::Names, Body::Tool),
        item(Kind::Tool, "mcp__gh__pr", Mode::Names, Body::Tool),
        item(Kind::Tool, "mcp__slack__post", Mode::Names, Body::Tool),
        item(Kind::Tool, "web_fetch", Mode::Names, Body::Tool),
        item(Kind::Skill, "pdf", Mode::Names, Body::Text(String::new())),
        item(
            Kind::Section,
            "secret",
            Mode::Deferred,
            Body::Text(String::new()),
        ),
    ];
    let text = index(&items).unwrap();
    assert!(
        text.contains("Deferred tools: web_fetch, mcp__gh__{issue, pr}, mcp__slack__post\n"),
        "{text}"
    );
    assert!(text.contains("Deferred skills: pdf\n"), "{text}");
    assert!(
        text.ends_with("1 more deferred items are searchable but not listed."),
        "{text}"
    );
    assert!(!text.contains("secret"));
}

#[test]
fn skill_body_strips_frontmatter() {
    assert_eq!(skill_body("---\nname: a\n---\nBody\n"), "Body\n");
    assert_eq!(skill_body("No frontmatter"), "No frontmatter");
    assert_eq!(skill_body("---\nunclosed"), "---\nunclosed");
}

#[test]
fn loaded_wraps_each_kind() {
    let skill = Body::Skill {
        file: "/s/pdf/SKILL.md".into(),
        base_dir: "/s/pdf".into(),
    };
    let text = loaded(
        &item(Kind::Skill, "pdf", Mode::Names, skill),
        Some("---\nx: 1\n---\nUse it"),
    );
    assert_eq!(
        text,
        "<skill name=\"pdf\" location=\"/s/pdf/SKILL.md\">\nReferences are relative to /s/pdf.\n\nUse it\n</skill>"
    );
    let context = item(
        Kind::Context,
        "/w/AGENTS.md",
        Mode::Deferred,
        Body::Text("rules".into()),
    );
    assert_eq!(
        loaded(&context, None),
        "<project_instructions path=\"/w/AGENTS.md\">\nrules\n</project_instructions>"
    );
    let section = item(
        Kind::Section,
        "mcp_servers",
        Mode::Names,
        Body::Text("list".into()),
    );
    assert_eq!(
        loaded(&section, None),
        "<mcp_servers>\nlist\n</mcp_servers>"
    );
    assert_eq!(
        loaded(&item(Kind::Tool, "t", Mode::Names, Body::Tool), None),
        "- t: desc"
    );
}

#[test]
fn no_match_counts_kinds() {
    assert_eq!(no_match("x", &[]), "Nothing is deferred.");
    let items = vec![
        item(Kind::Tool, "a", Mode::Names, Body::Tool),
        item(Kind::Tool, "b", Mode::Names, Body::Tool),
    ];
    assert_eq!(
        no_match("x", &items),
        "No deferred item matches `x`. Deferred: 2 tools. Try other keywords."
    );
}

#[test]
fn status_reports_rules_counts_and_loaded() {
    let layer = json!({ "tools": { "mcp__*": "names" }, "auto": false });
    let config = Config::merge([("g", Some(&layer))]);
    let sources = vec!["g".to_owned()];
    let text = status(&config, &sources, None, &[]);
    assert!(
        text.contains("auto: off")
            && text.contains("tools mcp__* = names")
            && text.contains("No run yet"),
        "{text}"
    );
    let plan = Plan {
        items: vec![item(Kind::Tool, "mcp__a__b", Mode::Names, Body::Tool)],
        off: vec![(Kind::Tool, "web".into(), 5)],
        ..Plan::default()
    };
    let text = status(&config, &sources, Some(&plan), &["mcp__x__y".to_owned()]);
    assert!(
        text.contains("tools: 1 names, 0 deferred, 1 off (~15 tokens)\n  mcp__a__b, web (off)"),
        "{text}"
    );
    assert!(
        text.contains("~15") && text.contains("loaded tools this session: mcp__x__y"),
        "{text}"
    );
}
