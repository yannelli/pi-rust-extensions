use super::*;
use serde_json::json;

#[test]
fn glob_matches_stars_and_marks() {
    assert!(glob("mcp__*", "mcp__github__create_issue"));
    assert!(glob("*issue", "mcp__github__create_issue"));
    assert!(glob("a?c", "abc"));
    assert!(glob("*", ""));
    assert!(glob("a*b*c", "axxbyyc"));
    assert!(!glob("a*b*c", "axxbyy"));
    assert!(!glob("bash", "bash2"));
}

#[test]
fn context_patterns_match_trailing_parts() {
    assert!(path_matches("AGENTS.md", "/w/AGENTS.md"));
    assert!(path_matches("docs/AGENTS.md", "/w/docs/AGENTS.md"));
    assert!(!path_matches("docs/AGENTS.md", "/w/AGENTS.md"));
    assert!(path_matches("/w/*", "/w/sub/CLAUDE.md"));
    assert!(!path_matches("/x/*", "/w/CLAUDE.md"));
}

#[test]
fn later_rules_and_layers_win() {
    let global = json!({ "tools": { "mcp__*": "names", "web_*": "off" }, "maxResults": 8 });
    let project = json!({ "tools": { "mcp__github__*": "on" }, "skills": { "*": "deferred" }, "auto": false });
    let config = Config::merge([("global", Some(&global)), ("project", Some(&project))]);
    assert_eq!(
        config.mode(Kind::Tool, "mcp__slack__post"),
        Some(Mode::Names)
    );
    assert_eq!(config.mode(Kind::Tool, "mcp__github__pr"), Some(Mode::On));
    assert_eq!(config.mode(Kind::Tool, "web_fetch"), Some(Mode::Off));
    assert_eq!(config.mode(Kind::Tool, "bash"), None);
    assert_eq!(config.mode(Kind::Skill, "pdf"), Some(Mode::Deferred));
    assert_eq!(config.mode(Kind::Section, "pdf"), None);
    assert_eq!((config.auto_percent, config.max_results), (None, 8));
    assert!(config.warnings.is_empty());
}

#[test]
fn bad_values_warn_and_keep_defaults() {
    let layer = json!({ "tools": { "a": "sometimes" }, "skills": [], "auto": -1, "maxResults": 0 });
    let config = Config::merge([("x", Some(&layer)), ("y", Some(&json!(3)))]);
    assert!(config.rules.is_empty());
    assert_eq!(config.auto_percent, Some(DEFAULT_AUTO_PERCENT));
    assert_eq!(config.max_results, DEFAULT_MAX_RESULTS);
    assert_eq!(config.warnings.len(), 4, "{:?}", config.warnings);
    assert!(config.warnings[0].starts_with("x: "));
}

#[test]
fn mode_aliases_parse() {
    assert_eq!(Mode::parse("name-only"), Some(Mode::Names));
    assert_eq!(Mode::parse(" Disabled "), Some(Mode::Off));
    assert_eq!(Mode::parse("defer"), Some(Mode::Deferred));
    assert_eq!(Mode::parse("x"), None);
    assert_eq!(Kind::parse("files"), Some(Kind::Context));
}

#[test]
fn edit_rule_sets_moves_and_removes() {
    let block = edit_rule(None, Kind::Tool, "mcp__*", Some(Mode::Names));
    assert_eq!(block, json!({ "tools": { "mcp__*": "names" } }));
    let block = edit_rule(Some(block), Kind::Tool, "bash", Some(Mode::On));
    let block = edit_rule(Some(block), Kind::Tool, "mcp__*", Some(Mode::Off));
    let keys: Vec<&String> = block["tools"].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["bash", "mcp__*"]);
    let block = edit_rule(Some(block), Kind::Tool, "bash", None);
    let block = edit_rule(Some(block), Kind::Tool, "mcp__*", None);
    assert_eq!(block, json!({}));
}

#[test]
fn read_file_skips_missing_and_broken() {
    let dir = tempfile::tempdir().unwrap();
    let path = project_path(dir.path());
    assert_eq!(read_file(&path), None);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{ nope").unwrap();
    assert_eq!(read_file(&path), None);
    std::fs::write(&path, r#"{"auto": 5}"#).unwrap();
    assert_eq!(read_file(&path), Some(json!({ "auto": 5 })));
}
