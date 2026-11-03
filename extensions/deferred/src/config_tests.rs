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

#[test]
fn user_mode_is_for_skills_only() {
    assert_eq!(Mode::parse("manual"), Some(Mode::User));
    assert_eq!(Mode::parse("User-Only"), Some(Mode::User));
    let layer = json!({ "skills": { "deploy": "user" }, "tools": { "bash": "user" } });
    let config = Config::merge([("x", Some(&layer))]);
    assert_eq!(config.mode(Kind::Skill, "deploy"), Some(Mode::User));
    assert_eq!(config.mode(Kind::Tool, "bash"), None);
    assert_eq!(
        config.warnings,
        ["x: `user` is for skills only, not `bash`"]
    );
}

#[test]
fn repo_rules_apply_after_global_and_before_project_rules() {
    let file = json!({ "tools": { "a": "names" }, "repos": { "Yannelli/Pi-Rust": { "tools": { "a": "off", "b": "off" } } } });
    let setting = json!({
        "tools": { "b": "names", "c": "names" },
        "repos": { "yannelli/*": { "tools": { "c": "deferred" }, "maxResults": 9 }, "other/x": { "tools": { "a": "on" } } }
    });
    let project_setting = json!({ "tools": { "c": "names" } });
    let project_file = json!({ "tools": { "z": "on" } });
    let layers = |repo| Layers {
        global_file: Some(("/g.json", &file)),
        setting: Some(&setting),
        project_setting: Some(&project_setting),
        project_file: Some(("/p.json", &project_file)),
        repo,
    };
    let config = Config::load(&layers(Some("yannelli/pi-rust")));
    assert_eq!(config.mode(Kind::Tool, "a"), Some(Mode::Off));
    assert_eq!(
        config.mode(Kind::Tool, "b"),
        Some(Mode::Off),
        "repo beats global"
    );
    assert_eq!(
        config.mode(Kind::Tool, "c"),
        Some(Mode::Names),
        "project beats repo"
    );
    assert_eq!(config.max_results, 9);
    let a = config.rule(Kind::Tool, "a").unwrap();
    assert_eq!(
        (a.source.as_str(), a.repo.as_deref()),
        ("/g.json", Some("Yannelli/Pi-Rust"))
    );
    let order: Vec<(&str, Option<&str>)> = config
        .rules
        .iter()
        .map(|r| (r.pattern.as_str(), r.repo.as_deref()))
        .collect();
    assert_eq!(
        order,
        [
            ("a", None),
            ("b", None),
            ("a", Some("Yannelli/Pi-Rust")),
            ("b", Some("Yannelli/Pi-Rust")),
            ("c", Some("yannelli/*")),
            ("c", None),
            ("z", None)
        ]
    );
    let elsewhere = Config::load(&layers(Some("someone/else")));
    assert_eq!(elsewhere.mode(Kind::Tool, "b"), Some(Mode::Names));
    assert!(elsewhere.rules.iter().all(|r| r.repo.is_none()));
    assert_eq!(Config::load(&layers(None)).rules.len(), 4);
}

#[test]
fn edit_repo_rule_reuses_the_key_and_prunes_empty_blocks() {
    let block = json!({ "tools": { "a": "off" }, "repos": { "Owner/Repo": { "skills": { "x": "user" } } } });
    let block = edit_repo_rule(Some(block), "owner/repo", Kind::Tool, "b", Some(Mode::Off));
    assert_eq!(
        block["repos"],
        json!({ "Owner/Repo": { "skills": { "x": "user" }, "tools": { "b": "off" } } })
    );
    let block = edit_repo_rule(Some(block), "owner/repo", Kind::Tool, "b", None);
    let block = edit_repo_rule(Some(block), "Owner/Repo", Kind::Skill, "x", None);
    assert_eq!(block, json!({ "tools": { "a": "off" } }));
}
