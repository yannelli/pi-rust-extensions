use pi_ext_sdk::Key;
use serde_json::json;

use crate::config::{
    Config, display_key, is_valid_key_spec, matches_spec, resolve_collapse_key, resolve_notes_key,
};

#[test]
fn key_spec_grammar() {
    assert!(is_valid_key_spec("ctrl+]"));
    assert!(is_valid_key_spec("ctrl+shift+h"));
    assert!(is_valid_key_spec("f9"));
    assert!(!is_valid_key_spec("ctr+]"));
    assert!(!is_valid_key_spec("ctrl+ctrl+a"));
    assert!(!is_valid_key_spec("ctrl+"));
    assert_eq!(resolve_collapse_key(Some(" ALT+O ")), "alt+o");
    assert_eq!(resolve_collapse_key(Some("OFF")), "off");
    assert_eq!(resolve_collapse_key(Some("ctr+]")), "ctrl+]");
    assert_eq!(resolve_notes_key(Some("т")), "т");
    assert_eq!(resolve_notes_key(Some("N")), "N");
    assert_eq!(resolve_notes_key(Some("enter")), "n");
    assert_eq!(resolve_notes_key(Some("Ctrl+N")), "ctrl+n");
    assert_eq!(display_key("ctrl+pagedown"), "Ctrl+PageDown");
    assert_eq!(display_key("alt+o"), "Alt+O");
}

#[test]
fn spec_matching() {
    assert!(matches_spec(&Key::parse("\x1d"), "ctrl+]"));
    assert!(matches_spec(&Key::parse("\x1b[93;5u"), "ctrl+]"));
    assert!(matches_spec(&Key::parse("\x0e"), "ctrl+n"));
    assert!(matches_spec(&Key::parse("\x1bo"), "alt+o"));
    assert!(!matches_spec(&Key::parse("]"), "ctrl+]"));
    assert!(!matches_spec(&Key::parse("\x1d"), "off"));
}

#[test]
fn config_values_and_merge() {
    let config = Config::from_value(&json!({
        "collapseKey": 5, "notesKey": "x",
        "guidance": { "promptSnippet": "snip", "promptGuidelines": ["a", 1], "description": "" }
    }));
    assert_eq!(config.collapse_key, "ctrl+]");
    assert_eq!(config.notes_key, "x");
    assert_eq!(config.guidance.prompt_snippet.as_deref(), Some("snip"));
    assert_eq!(config.guidance.prompt_guidelines, None);
    assert_eq!(config.guidance.description, None);

    let root = std::env::temp_dir().join(format!("aq-config-{}", std::process::id()));
    let agent = root.join("agent");
    let cwd = root.join("project");
    std::fs::create_dir_all(agent.join("ext")).unwrap();
    std::fs::create_dir_all(cwd.join(".pi/ext")).unwrap();
    std::fs::write(
        agent.join("ext/ask-user-question.json"),
        r#"{"collapseKey":"alt+o","notesKey":"m"}"#,
    )
    .unwrap();
    std::fs::write(
        cwd.join(".pi/ext/ask-user-question.json"),
        r#"{"notesKey":"k"}"#,
    )
    .unwrap();
    let loaded = Config::load(&agent, &cwd);
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(loaded.collapse_key, "alt+o");
    assert_eq!(loaded.notes_key, "k");
}
