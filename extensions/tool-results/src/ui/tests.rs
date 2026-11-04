use pi_ext_sdk::Key;
use serde_json::json;

use super::*;
use crate::config::Config;
use crate::stats::Stats;
use render::Inputs;

fn config() -> Config {
    Config::from_value(&json!({ "tools": { "mcp__*": { "json": "table" } } }))
}

fn field_index(key: &str) -> usize {
    FIELDS.iter().position(|f| f.key == key).unwrap()
}

#[test]
fn toggles_and_cycles_settings() {
    let config = config();
    let mut view = View::new();
    view.selected = field_index("dedupe");
    assert_eq!(
        view.key(&Key::Char(' '), &config, 0),
        Action::Set {
            rule: None,
            key: "dedupe",
            value: Some(json!(false))
        }
    );
    view.selected = field_index("json");
    assert_eq!(
        view.key(&Key::Right, &config, 0),
        Action::Set {
            rule: None,
            key: "json",
            value: Some(json!("compact"))
        }
    );
    assert_eq!(
        view.key(&Key::Left, &config, 0),
        Action::Set {
            rule: None,
            key: "json",
            value: Some(json!("off"))
        }
    );
    view.selected = field_index("threshold");
    assert_eq!(
        view.key(&Key::Enter, &config, 0),
        Action::EditNumber {
            rule: None,
            key: "threshold"
        }
    );
    assert_eq!(view.key(&Key::Right, &config, 0), Action::Redraw);
}

#[test]
fn rules_tab_opens_adds_and_removes() {
    let config = config();
    let mut view = View::new();
    view.key(&Key::Tab, &config, 0);
    assert_eq!(view.tab(), Tab::Rules);
    assert_eq!(patterns(&config), ["read", "mcp__*"]);
    assert_eq!(view.key(&Key::Char('d'), &config, 0), Action::Redraw);
    assert!(view.status.as_deref().unwrap().contains("built-in rule"));
    view.key(&Key::Down, &config, 0);
    assert_eq!(
        view.key(&Key::Char('d'), &config, 0),
        Action::RemoveRule("mcp__*".into())
    );
    view.key(&Key::Down, &config, 0);
    assert_eq!(view.key(&Key::Enter, &config, 0), Action::AddRule);
    view.selected = 1;
    view.key(&Key::Enter, &config, 0);
    assert_eq!(view.screen, Screen::Rule("mcp__*".into()));
}

#[test]
fn rule_fields_cycle_through_inherit() {
    let config = config();
    let mut view = View::new();
    view.screen = Screen::Rule("mcp__*".into());
    let fields = rule_fields();
    view.selected = fields.iter().position(|f| f.key == "json").unwrap();
    assert_eq!(
        view.key(&Key::Right, &config, 0),
        Action::Set {
            rule: Some("mcp__*".into()),
            key: "json",
            value: Some(json!("off"))
        }
    );
    view.selected = fields.iter().position(|f| f.key == "ansi").unwrap();
    assert_eq!(
        view.key(&Key::Right, &config, 0),
        Action::Set {
            rule: Some("mcp__*".into()),
            key: "ansi",
            value: Some(json!(true))
        }
    );
    view.selected = fields.iter().position(|f| f.key == "threshold").unwrap();
    assert_eq!(
        view.key(&Key::Enter, &config, 0),
        Action::EditNumber {
            rule: Some("mcp__*".into()),
            key: "threshold"
        }
    );
    assert_eq!(view.key(&Key::Escape, &config, 0), Action::Redraw);
    assert_eq!(view.screen, Screen::List);
    assert_eq!(view.key(&Key::Escape, &config, 0), Action::Close);
}

#[test]
fn renders_each_tab_within_the_width() {
    let config = config();
    let mut stats = Stats::default();
    let outcome = crate::pipeline::Outcome {
        before: 5000,
        after: 1000,
        ..Default::default()
    };
    stats.record("bash", Some(&outcome), 5000, false);
    let inputs = Inputs {
        config: &config,
        stats: &stats,
        note: None,
    };
    let mut view = View::new();
    let lines = view.render(&inputs, 60, Some(20));
    assert!(lines.len() <= 20, "{}", lines.len());
    assert!(
        lines
            .iter()
            .all(|l| pi_ext_sdk::style::visible_width(l) <= 60)
    );
    let plain: Vec<String> = lines.iter().map(|l| pi_ext_sdk::style::strip(l)).collect();
    assert!(
        plain
            .iter()
            .any(|l| l.contains("Enabled") && l.ends_with("on")),
        "{plain:?}"
    );
    view.tab = 1;
    let plain: Vec<String> = view
        .render(&inputs, 120, None)
        .iter()
        .map(|l| pi_ext_sdk::style::strip(l))
        .collect();
    assert!(
        plain
            .iter()
            .any(|l| l.contains("mcp__*") && l.contains("json=table")),
        "{plain:?}"
    );
    assert!(
        plain
            .iter()
            .any(|l| l.contains("read") && l.contains("built-in")),
        "{plain:?}"
    );
    view.tab = 2;
    let plain: Vec<String> = view
        .render(&inputs, 120, None)
        .iter()
        .map(|l| pi_ext_sdk::style::strip(l))
        .collect();
    assert!(
        plain
            .iter()
            .any(|l| l.contains("bash") && l.contains("4k now")),
        "{plain:?}"
    );
    assert!(
        plain.iter().any(|l| l.contains("Last: bash 5,000 → 1,000")),
        "{plain:?}"
    );
    view.screen = Screen::Rule("mcp__*".into());
    let plain: Vec<String> = view
        .render(&inputs, 120, None)
        .iter()
        .map(|l| pi_ext_sdk::style::strip(l))
        .collect();
    assert!(
        plain
            .iter()
            .any(|l| l.contains("Strip ANSI") && l.contains("inherit (on)")),
        "{plain:?}"
    );
}

#[test]
fn clamp_keeps_the_selection_in_range() {
    let config = config();
    let mut view = View::new();
    view.tab = 1;
    view.selected = 9;
    view.clamp(&config, 0);
    assert_eq!(view.selected, 2);
}
