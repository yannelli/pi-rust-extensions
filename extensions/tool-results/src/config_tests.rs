use serde_json::json;

use super::*;
use crate::files;

#[test]
fn defaults() {
    let config = Config::default();
    assert_eq!(config.base, Settings::default());
    assert_eq!(config.base.threshold, 20_000);
    assert_eq!(config.base.json, JsonFormat::Toon);
    assert_eq!((config.context_chars, config.max_matches), (200, 20));
    assert!(!config.dry_run);
    assert_eq!(config.rules.len(), 1);
    assert!(config.rules[0].builtin);
}

#[test]
fn parses_keys_and_ignores_bad_values() {
    let config = Config::from_value(&json!({
        "threshold": 5000, "json": "table", "dedupe": false, "errors": "all",
        "headChars": "x", "longLineChars": 3, "dryRun": true, "maxMatches": 0, "nope": 1
    }));
    assert_eq!(config.base.threshold, 5000);
    assert_eq!(config.base.json, JsonFormat::Table);
    assert!(!config.base.dedupe);
    assert_eq!(config.base.errors, ErrorMode::All);
    assert_eq!(config.base.head_chars, 4000);
    assert_eq!(config.base.long_line_chars, 80);
    assert_eq!(config.max_matches, 1);
    assert!(config.dry_run);
    assert_eq!(Config::from_value(&json!([1])), Config::default());
}

#[test]
fn per_tool_rules_with_globs() {
    let config = Config::from_value(&json!({
        "dedupe": true,
        "tools": {
            "mcp__*": { "json": "compact", "dedupe": false },
            "mcp__github__*": { "json": "toon" },
            "bash": { "enabled": false },
            "bad": 3
        }
    }));
    assert_eq!(config.for_tool("mcp__github__list").json, JsonFormat::Toon);
    assert!(!config.for_tool("mcp__github__list").dedupe);
    assert_eq!(
        config.for_tool("mcp__slack__post").json,
        JsonFormat::Compact
    );
    assert!(!config.for_tool("bash").enabled);
    assert!(config.for_tool("grep").enabled && config.for_tool("grep").dedupe);
    assert!(!config.for_tool("read").whitespace);
    assert_eq!(config.for_tool("read").json, JsonFormat::Off);
    assert!(config.rules.iter().all(|r| r.pattern != "bad"));
    let plain = Config::from_value(&json!({ "defaultRules": false }));
    assert!(plain.for_tool("read").whitespace);
    let user = Config::from_value(&json!({ "tools": { "read": { "whitespace": true } } }));
    assert!(user.for_tool("read").whitespace);
}

#[test]
fn glob_matching() {
    assert!(glob("*", "anything"));
    assert!(glob("mcp__*", "mcp__x__y"));
    assert!(glob("b?sh", "bash"));
    assert!(glob("*_get", "tool_result_get"));
    assert!(!glob("bash", "bash2"));
    assert!(!glob("mcp__*", "xmcp__"));
    assert!(glob("a*b*c", "aXXbYYc"));
}

#[test]
fn files_load_save_and_merge() {
    let dir = tempfile::tempdir().unwrap();
    let global = files::global_path(dir.path());
    assert_eq!(files::read_file(&global), None);
    files::edit(&global, None, "threshold", Some(json!(9000))).unwrap();
    files::edit(&global, Some("bash"), "dedupe", Some(json!(false))).unwrap();
    files::edit(&global, Some("bash"), "json", Some(json!("off"))).unwrap();
    files::edit(&global, Some("bash"), "json", None).unwrap();
    files::edit_rule(&global, "mcp__*", false).unwrap();
    let saved = files::read_file(&global).unwrap();
    assert_eq!(
        saved,
        json!({ "threshold": 9000, "tools": { "bash": { "dedupe": false }, "mcp__*": {} } })
    );
    files::edit_rule(&global, "mcp__*", true).unwrap();
    let project = json!({ "threshold": 100, "tools": { "bash": { "dedupe": true } } });
    let config = files::load(files::read_file(&global), Some(project));
    assert_eq!(config.base.threshold, 500);
    assert!(config.for_tool("bash").dedupe);
    let config = files::load(files::read_file(&global), None);
    assert_eq!(config.base.threshold, 9000);
    assert!(!config.for_tool("bash").dedupe);
    std::fs::write(&global, "{ nope").unwrap();
    assert_eq!(files::read_file(&global), Some(json!({})));
}
