use std::collections::BTreeSet;

use pi_ext_sdk::{Key, style};
use serde_json::{Value, json};

use super::rows::{Sources, Why, build};
use super::*;
use crate::config::{Config, SETTING_SOURCE};

fn inventory(setting: Value, project: Option<Value>) -> Inventory {
    let global_file = json!({ "tools": { "web_*": "off" } });
    let config = Config::merge([
        ("/a/ext/deferred.json", Some(&global_file)),
        (SETTING_SOURCE, Some(&setting)),
    ]);
    let tool = |name: &str, path: &str, exposure: &str| json!({ "name": name, "description": format!("{name} tool"), "exposure": exposure, "parameters": {}, "sourceInfo": { "path": path, "source": "builtin" } });
    let tools = vec![
        tool("read", "builtin:read", "direct"),
        tool("mcp__gh__issue", "builtin:mcp", "direct"),
        tool("mcp__gh__pr", "builtin:mcp", "codemode"),
        tool("web_fetch", "/ext/web", "direct"),
        tool("deferred_search", "/ext/deferred", "direct"),
    ];
    let options = json!({
        "skills": [{ "name": "pdf", "description": "PDF files", "filePath": "/s/pdf/SKILL.md" },
                   { "name": "manual", "description": "m", "filePath": "/s/m/SKILL.md", "disableModelInvocation": true }],
        "contextFiles": [{ "path": "/w/AGENTS.md", "content": "# Rules\nTabs." }],
        "sections": { "mcp_servers": "gh", "deferred": "ours" }
    });
    let loaded = BTreeSet::from(["mcp__gh__issue".to_owned()]);
    let global = Some(setting.clone());
    build(&Sources {
        config: &config,
        tools: &tools,
        options: &options,
        context_window: None,
        loaded: &loaded,
        global: global.as_ref(),
        project: project.as_ref(),
    })
}

fn setting() -> Value {
    json!({ "tools": { "mcp__*": "names", "read": "deferred" }, "skills": { "pdf": "names" } })
}

fn names(view: &View, inv: &Inventory) -> Vec<String> {
    view.entries(inv)
        .iter()
        .map(|e| match e {
            Entry::Item(r) => r.name.clone(),
            Entry::Rule(r) => r.pattern.clone(),
        })
        .collect()
}

#[test]
fn inventory_explains_each_mode() {
    let inv = inventory(setting(), Some(json!({ "tools": { "read": "deferred" } })));
    let row = |name: &str| inv.rows.iter().find(|r| r.name == name).unwrap();
    assert_eq!(
        row("read").why,
        Why::Rule {
            pattern: "read".into(),
            source: SETTING_SOURCE.into(),
            scope: Some(Scope::Project)
        }
    );
    assert_eq!(row("read").exact_scope(), Some(Scope::Project));
    assert!(matches!(&row("mcp__gh__issue").why, Why::Rule { pattern, .. } if pattern == "mcp__*"));
    assert!(row("mcp__gh__issue").loaded && row("mcp__gh__issue").exact_scope().is_none());
    assert_eq!(
        row("web_fetch").why,
        Why::Rule {
            pattern: "web_*".into(),
            source: "/a/ext/deferred.json".into(),
            scope: None
        }
    );
    assert!(row("manual").pinned() && row("manual").mode == Mode::On);
    assert!(
        inv.rows
            .iter()
            .all(|r| r.name != "deferred_search" && r.name != "deferred")
    );
    assert_eq!(
        (
            inv.count(Kind::Tool),
            inv.count(Kind::Skill),
            inv.count(Kind::Context),
            inv.count(Kind::Section)
        ),
        (4, 2, 1, 1)
    );
    let web = inv.rules.iter().find(|r| r.pattern == "web_*").unwrap();
    assert_eq!((web.scope, web.matches), (None, 1));
    assert_eq!(
        inv.rules
            .iter()
            .find(|r| r.pattern == "mcp__*")
            .unwrap()
            .matches,
        2
    );
}

#[test]
fn typing_filters_tab_switches_and_esc_clears_then_closes() {
    let inv = inventory(setting(), None);
    let mut view = View::new(false);
    for c in "gh".chars() {
        view.key(&Key::Char(c), &inv);
    }
    assert_eq!(names(&view, &inv), ["mcp__gh__issue", "mcp__gh__pr"]);
    view.key(&Key::Down, &inv);
    view.key(&Key::Down, &inv);
    assert_eq!(view.selected, 0, "down wraps");
    assert_eq!(view.key(&Key::Escape, &inv), Action::Redraw);
    assert!(view.query.is_empty());
    view.key(&Key::Tab, &inv);
    assert_eq!(view.tab(), Tab::Kind(Kind::Skill));
    view.key(&Key::BackTab, &inv);
    view.key(&Key::BackTab, &inv);
    assert_eq!(view.tab(), Tab::Rules);
    assert_eq!(view.key(&Key::Escape, &inv), Action::Close);
}

#[test]
fn arrows_cycle_modes_at_the_rule_scope() {
    let inv = inventory(setting(), Some(json!({ "tools": { "read": "deferred" } })));
    let mut view = View::new(true);
    let write = |pattern: &str, mode: Mode, scope: Scope| Action::Write {
        change: Change {
            kind: Kind::Tool,
            pattern: pattern.into(),
            mode: Some(mode),
        },
        scope,
    };
    assert_eq!(
        view.key(&Key::Right, &inv),
        write("read", Mode::Off, Scope::Project)
    );
    assert_eq!(
        view.key(&Key::Left, &inv),
        write("read", Mode::Names, Scope::Project)
    );
    view.key(&Key::Down, &inv);
    assert_eq!(
        view.key(&Key::Right, &inv),
        write("mcp__gh__issue", Mode::Deferred, Scope::Global)
    );
    view.selected = 3;
    assert_eq!(
        view.key(&Key::Right, &inv),
        write("web_fetch", Mode::On, Scope::Global)
    );
}

#[test]
fn menu_asks_for_a_scope_when_the_project_is_trusted() {
    let inv = inventory(setting(), None);
    let mut view = View::new(true);
    view.selected = 1;
    view.key(&Key::Enter, &inv);
    assert_eq!(
        view.screen,
        Screen::Menu { choice: 1 },
        "the cursor starts on the current mode"
    );
    view.key(&Key::Up, &inv);
    assert_eq!(
        view.choices(&inv),
        [
            Choice::Set(Mode::On),
            Choice::Set(Mode::Names),
            Choice::Set(Mode::Deferred),
            Choice::Set(Mode::Off),
            Choice::Unload
        ]
    );
    view.key(&Key::Enter, &inv);
    assert!(
        matches!(&view.screen, Screen::Scope { change, choice: 0 } if change.pattern == "mcp__gh__issue")
    );
    view.key(&Key::Down, &inv);
    let action = view.key(&Key::Enter, &inv);
    assert_eq!(
        action,
        Action::Write {
            change: Change {
                kind: Kind::Tool,
                pattern: "mcp__gh__issue".into(),
                mode: Some(Mode::On)
            },
            scope: Scope::Project
        }
    );
    view.key(&Key::Enter, &inv);
    view.key(&Key::Up, &inv);
    view.key(&Key::Up, &inv);
    assert_eq!(
        view.key(&Key::Enter, &inv),
        Action::Unload("mcp__gh__issue".into())
    );
}

#[test]
fn pinned_items_and_file_rules_are_locked() {
    let inv = inventory(setting(), None);
    let mut view = View::new(false);
    view.tab = 1;
    view.selected = 1;
    assert!(view.choices(&inv).is_empty());
    assert_eq!(view.key(&Key::Right, &inv), Action::Redraw);
    assert!(
        view.status
            .as_ref()
            .unwrap()
            .1
            .contains("disable-model-invocation")
    );
    view.tab = 4;
    view.selected = inv.rules.iter().position(|r| r.pattern == "web_*").unwrap();
    view.key(&Key::Enter, &inv);
    assert_eq!(view.screen, Screen::List);
    assert!(
        view.status
            .as_ref()
            .unwrap()
            .1
            .contains("/a/ext/deferred.json")
    );
}

#[test]
fn render_fits_the_width_and_shows_the_acp_chrome() {
    let inv = inventory(setting(), None);
    let mut view = View::new(false);
    let lines = view.render(&inv, 60, Some(20));
    assert!(lines.len() <= 20 && lines.iter().all(|l| style::visible_width(l) <= 60));
    let plain: Vec<String> = lines.iter().map(|l| style::strip(l)).collect();
    assert!(plain[1].starts_with(" Deferred context"), "{plain:?}");
    assert!(
        plain[2].starts_with(" [Tools (4)]  Skills (2)"),
        "{plain:?}"
    );
    assert!(
        plain
            .iter()
            .any(|l| l.starts_with("→ ◐ read") || l.starts_with("→ ○ read")),
        "{plain:?}"
    );
    assert!(
        plain
            .iter()
            .any(|l| l.contains("↑↓ select · ←→ change mode · enter actions")),
        "{plain:?}"
    );
    view.key(&Key::Enter, &inv);
    let plain: Vec<String> = view
        .render(&inv, 80, None)
        .iter()
        .map(|l| style::strip(l))
        .collect();
    assert!(
        plain
            .iter()
            .any(|l| l.contains("Mode: deferred (rule `read` in global settings)")),
        "{plain:?}"
    );
    assert!(
        plain
            .iter()
            .any(|l| l.contains("→ Searchable, not listed (deferred) (current)")),
        "{plain:?}"
    );
    assert!(
        plain
            .iter()
            .any(|l| l.contains("Searchable, not listed (deferred) (current)")),
        "{plain:?}"
    );
}
