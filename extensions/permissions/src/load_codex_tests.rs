//! Which Codex rules files load, in which order, and what a file Codex cannot read does, as in openai/codex
//! 67f0994b `core/src/exec_policy.rs` (`policy_files`, `collect_policy_files`, `load_exec_policy`).

use serde_json::json;

use super::tests::{fixture, sources, write};
use super::*;
use crate::evaluate::{Call, evaluate};

const RM: &str = "prefix_rule(pattern=[\"rm\"], decision=\"forbidden\")";

fn codex_files(loaded: &Loaded) -> Vec<(Source, String)> {
    let codex = [Source::CodexSystem, Source::CodexUser, Source::CodexProject];
    let rules = loaded
        .rules
        .iter()
        .filter(|rule| codex.contains(&rule.source));
    let name = |rule: &Rule| {
        rule.file
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    };
    rules.map(|rule| (rule.source, name(rule))).collect()
}

fn fetch(loaded: &Loaded, url: &str) -> Option<Decision> {
    let input = json!({ "url": url });
    let call = Call {
        tool: "fetch",
        input: &input,
        cwd: &loaded.cwd,
        home: &loaded.cwd,
        dirs: &loaded.dirs,
    };
    evaluate(&loaded.rules, &call).decision
}

fn bash_call<'a>(loaded: &'a Loaded, input: &'a serde_json::Value) -> Call<'a> {
    Call {
        tool: "bash",
        input,
        cwd: &loaded.cwd,
        home: &loaded.cwd,
        dirs: &loaded.dirs,
    }
}

/// Turns `keys` off in the pi global settings, as `/permissions` sources does.
fn sources_off(fx: &super::tests::Fixture, keys: &[&str]) {
    let off: serde_json::Map<String, serde_json::Value> = keys
        .iter()
        .map(|k| ((*k).to_owned(), json!(false)))
        .collect();
    write(
        &global_file(&fx.agent),
        &json!({ "sources": off }).to_string(),
    );
}

#[test]
fn system_rules_load_first_by_name_without_a_system_config_toml() {
    let fx = fixture();
    write(&fx.paths.codex_system.join("rules/b.rules"), RM);
    write(&fx.paths.codex_system.join("rules/a.rules"), RM);
    fx.user_rules("u.rules", RM);
    write(&fx.cwd.join(".codex/rules/p.rules"), RM);
    assert!(!fx.paths.codex_system.join("config.toml").exists());
    let loaded = fx.load(true);
    let expected = [
        (Source::CodexSystem, "a.rules"),
        (Source::CodexSystem, "b.rules"),
        (Source::CodexUser, "u.rules"),
        (Source::CodexProject, "p.rules"),
    ];
    assert_eq!(
        codex_files(&loaded),
        expected.map(|(s, n)| (s, n.to_owned()))
    );
    assert!(
        loaded.codex_unreadable.is_empty() && loaded.blocked().is_none(),
        "{:?}",
        loaded.notes
    );
}

#[test]
fn the_last_network_rule_in_load_order_wins_across_system_user_and_project() {
    let fx = fixture();
    let rule = |decision: &str| {
        format!("network_rule(host=\"x.example\", protocol=\"https\", decision=\"{decision}\")")
    };
    write(
        &fx.paths.codex_system.join("rules/net.rules"),
        &rule("allow"),
    );
    fx.user_rules("net.rules", &rule("deny"));
    assert_eq!(
        fetch(&fx.load(true), "https://x.example/"),
        Some(Decision::Deny),
        "user after system"
    );
    write(&fx.cwd.join(".codex/rules/net.rules"), &rule("allow"));
    assert_eq!(
        fetch(&fx.load(true), "https://x.example/"),
        Some(Decision::Allow),
        "project after user"
    );
    assert_eq!(
        fetch(&fx.load(false), "https://x.example/"),
        Some(Decision::Deny),
        "untrusted: no project rules"
    );
}

#[test]
fn an_untrusted_project_keeps_system_and_user_rules_and_reads_no_project_file() {
    let fx = fixture();
    write(&fx.paths.codex_system.join("rules/s.rules"), RM);
    fx.user_rules("u.rules", RM);
    std::fs::create_dir_all(fx.cwd.join(".codex")).unwrap();
    std::fs::write(
        fx.cwd.join(".codex/rules"),
        "a file where the folder should be",
    )
    .unwrap();
    let loaded = fx.load(false);
    let found = sources(&loaded);
    assert!(
        found.contains(&Source::CodexSystem) && found.contains(&Source::CodexUser),
        "{found:?}"
    );
    assert!(!found.contains(&Source::CodexProject));
    assert!(
        loaded.blocked().is_none(),
        "an untrusted project folder is not read: {:?}",
        loaded.notes
    );
}

#[test]
fn user_and_project_sources_off_keep_system_and_managed_rules_and_read_neither() {
    let fx = fixture();
    fx.all_sources();
    write(&fx.paths.codex_system.join("rules/s.rules"), RM);
    fx.user_rules(
        "zz-bad.rules",
        r#"network_rule(host="*", protocol="https", decision="deny")"#,
    );
    std::fs::write(
        fx.cwd.join(".codex/rules/zz-unreadable.rules"),
        [0xff, 0xfe],
    )
    .unwrap();
    sources_off(&fx, &["codex-user", "codex-project"]);
    let loaded = fx.load(true);
    let found = sources(&loaded);
    assert!(
        found.contains(&Source::CodexSystem) && found.contains(&Source::CodexManaged),
        "{found:?}"
    );
    assert!(!found.contains(&Source::CodexUser) && !found.contains(&Source::CodexProject));
    assert!(
        loaded.blocked().is_none() && loaded.codex_failed.is_empty(),
        "{:?}",
        loaded.notes
    );
}

#[test]
fn a_parse_error_drops_system_user_and_project_rules_and_keeps_managed_ones() {
    let fx = fixture();
    fx.all_sources();
    write(&fx.paths.codex_system.join("rules/s.rules"), RM);
    fx.user_rules(
        "zz-bad.rules",
        r#"network_rule(host="*", protocol="https", decision="deny")"#,
    );
    let found = sources(&fx.load(true));
    for dropped in [Source::CodexSystem, Source::CodexUser, Source::CodexProject] {
        assert!(!found.contains(&dropped), "{dropped:?} still applies");
    }
    assert!(
        found.contains(&Source::CodexManaged) && found.contains(&Source::PiGlobal),
        "{found:?}"
    );
}

#[test]
fn an_unreadable_rules_file_blocks_every_call_until_it_is_fixed_or_its_source_is_off() {
    let fx = fixture();
    write(&global_file(&fx.agent), r#"{"allow": ["Bash(ls *)"]}"#);
    let bad = fx.paths.codex_home.join("rules/bad.rules");
    write(&bad, "");
    std::fs::write(&bad, [0xff, 0xfe]).unwrap();
    let loaded = fx.load(true);
    let reason = loaded
        .blocked()
        .expect("invalid UTF-8 cannot be read as a rules file");
    assert!(reason.contains(&bad.display().to_string()), "{reason}");
    assert!(
        loaded
            .notes
            .iter()
            .any(|note| note.starts_with("cannot read Codex rules")),
        "{:?}",
        loaded.notes
    );
    let input = json!({ "command": "ls" });
    let lines = crate::listing::explain(&loaded, &bash_call(&loaded, &input));
    assert_eq!(
        lines,
        [format!("bash: blocked ({reason})")],
        "the sample test shows the block, not the pi allow"
    );
    sources_off(&fx, &["codex-user"]);
    assert!(
        fx.load(true).blocked().is_none(),
        "a source that is off is not read"
    );
    write(&global_file(&fx.agent), r#"{"allow": ["Bash(ls *)"]}"#);
    write(&bad, RM);
    assert!(
        fx.load(true).blocked().is_none(),
        "a fixed file loads again"
    );
}

#[test]
fn a_rules_path_that_is_not_a_folder_blocks_and_a_missing_folder_does_not() {
    let fx = fixture();
    assert!(
        fx.load(true).blocked().is_none(),
        "no system, user, or project rules folder"
    );
    write(
        &fx.paths.codex_system.join("rules"),
        "a file where the folder should be",
    );
    let loaded = fx.load(true);
    let rules_dir = fx.paths.codex_system.join("rules").display().to_string();
    assert!(
        loaded
            .codex_unreadable
            .iter()
            .any(|what| what.starts_with(&rules_dir)),
        "{:?}",
        loaded.codex_unreadable
    );
}

#[cfg(unix)]
#[test]
fn symlinks_and_folders_named_rules_are_skipped() {
    let fx = fixture();
    let outside = fx.paths.home.join("outside.rules");
    write(&outside, RM);
    let rules = fx.paths.codex_home.join("rules");
    std::fs::create_dir_all(rules.join("folder.rules")).unwrap();
    std::os::unix::fs::symlink(&outside, rules.join("link.rules")).unwrap();
    let loaded = fx.load(true);
    assert!(
        codex_files(&loaded).is_empty(),
        "{:?}",
        codex_files(&loaded)
    );
    assert!(loaded.blocked().is_none(), "{:?}", loaded.notes);
}
