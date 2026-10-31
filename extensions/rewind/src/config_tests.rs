use super::*;

#[test]
fn defaults_match_upstream() {
    let config = Config::default();
    assert!(config.checkpoints_on());
    assert!(!config.restore_on_resume && !config.restore_on_fork && !config.restore_on_clone);
    assert_eq!(config.restore_on_tree, RestoreOnTree::Ask);
    assert_eq!(config.exclude.len(), DEFAULT_EXCLUDE_PATTERNS.len());
    assert_eq!(config.max_file_bytes, None);
}

#[test]
fn reads_upstream_keys() {
    let config = Config::from_value(&json!({
        "rewind": { "restoreOnTree": "always" },
        "checkpoint": {
            "restoreOnResume": true, "exclude": ["secrets/"], "include": ["dist/keep.js", "!a"],
            "maxFileMB": 0.5, "autoCheckpoint": false
        }
    }));
    assert_eq!(config.restore_on_tree, RestoreOnTree::Always);
    assert!(config.restore_on_resume);
    assert!(!config.checkpoints_on());
    assert_eq!(config.max_file_bytes, Some(512 * 1024));
    let tail = &config.exclude[DEFAULT_EXCLUDE_PATTERNS.len()..];
    assert_eq!(tail, ["secrets/", "!dist/keep.js", "!a"]);
}

#[test]
fn bad_values_fall_back() {
    let config = Config::from_value(&json!({
        "rewind": { "restoreOnTree": "sometimes" },
        "checkpoint": { "maxFileMB": -1, "restoreOnFork": "yes" }
    }));
    assert_eq!(config.restore_on_tree, RestoreOnTree::Ask);
    assert_eq!(config.max_file_bytes, None);
    assert!(!config.restore_on_fork);
}

#[test]
fn project_layer_wins_per_key() {
    let merged = merge_layers(&[
        json!({ "rewind": { "restoreOnTree": "never" }, "checkpoint": { "exclude": ["a"], "restoreOnFork": true } }),
        Value::Null,
        json!({ "checkpoint": { "exclude": ["b"], "restoreOnFork": null } }),
    ]);
    let config = Config::from_value(&merged);
    assert_eq!(config.restore_on_tree, RestoreOnTree::Never);
    assert!(config.restore_on_fork);
    assert_eq!(config.exclude.last().map(String::as_str), Some("b"));
}

#[test]
fn untrusted_project_cannot_trigger_restores() {
    let layer = json!({
        "rewind": { "restoreOnTree": "always" },
        "checkpoint": { "restoreOnResume": true, "exclude": ["x"] }
    });
    let (kept, dropped) = untrusted_project(layer);
    assert!(dropped);
    assert_eq!(
        kept,
        json!({ "rewind": {}, "checkpoint": { "exclude": ["x"] } })
    );
    let (kept, dropped) = untrusted_project(json!({ "rewind": { "restoreOnTree": "never" } }));
    assert!(!dropped);
    assert_eq!(kept["rewind"]["restoreOnTree"], "never");
}

#[test]
fn untrusted_merged_settings_keep_global_restore_keys() {
    let merged = json!({
        "rewind": { "restoreOnTree": "always" },
        "checkpoint": { "restoreOnResume": true, "restoreOnFork": true, "exclude": ["p"] }
    });
    let global_settings = json!({
        "rewind": { "restoreOnTree": "never" },
        "checkpoint": { "restoreOnFork": true, "exclude": ["g"] }
    });
    let (layers, dropped) = trusted_layers(
        merged.clone(),
        global_settings,
        Value::Null,
        Value::Null,
        false,
    );
    assert!(dropped);
    let config = Config::from_value(&merge_layers(&layers));
    assert!(!config.restore_on_resume);
    assert!(config.restore_on_fork);
    assert_eq!(config.restore_on_tree, RestoreOnTree::Never);
    assert_eq!(config.exclude.last().map(String::as_str), Some("p"));

    let (layers, dropped) = trusted_layers(merged, Value::Null, Value::Null, Value::Null, true);
    assert!(!dropped);
    let config = Config::from_value(&merge_layers(&layers));
    assert!(config.restore_on_resume);
    assert_eq!(config.restore_on_tree, RestoreOnTree::Always);
}

#[test]
fn malformed_files_are_reported() {
    let dir = crate::testdir::TestDir::new("config");
    let path = dir.path().join("rewind.json");
    assert_eq!(read_json(&path), Ok(Value::Null));
    std::fs::write(&path, "{ bad").unwrap();
    assert!(
        read_json(&path)
            .unwrap_err()
            .starts_with("Ignored malformed")
    );
}

#[test]
fn durations_fall_back_on_values_that_do_not_fit() {
    use super::{DEFAULT_GIT_TIMEOUT, DEFAULT_STAGE_BUDGET};
    use std::time::Duration;
    let parse = |budget: Value, timeout: Value| {
        let config = Config::from_value(&json!({
            "checkpoint": { "stageBudgetSeconds": budget, "gitTimeoutSeconds": timeout }
        }));
        (config.stage_budget, config.git_timeout)
    };
    let defaults = (DEFAULT_STAGE_BUDGET, DEFAULT_GIT_TIMEOUT);
    assert_eq!(parse(json!(1e308), json!(1e300)), defaults);
    assert_eq!(parse(json!(-1), json!(0)), defaults);
    assert_eq!(parse(json!("10"), json!(null)), defaults);
    let fractional = (Duration::from_millis(250), Duration::from_secs(30));
    assert_eq!(parse(json!(0.25), json!(30)), fractional);
}
