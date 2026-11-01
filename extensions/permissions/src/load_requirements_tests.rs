//! Codex requirements files as openai/codex 67f0994b `config/src/loader` reads them: only a missing file is
//! skipped, any other read, TOML, or rule error blocks, and the legacy `managed_config.toml` gives no rules.

use std::path::PathBuf;

use super::tests::{Fixture, fixture, write};
use super::*;

const PUSH: &str = "[rules]\nprefix_rules = [{ pattern = [{ token = \"git\" }, { token = \"push\" }], decision = \"forbidden\" }]\n";
const RM: &str =
    "[rules]\nprefix_rules = [{ pattern = [{ token = \"rm\" }], decision = \"forbidden\" }]\n";

/// The fixture with the legacy `managed_config.toml` after `requirements.toml`, as on Unix.
fn with_legacy() -> (Fixture, PathBuf, PathBuf) {
    let mut fx = fixture();
    let requirements = fx.paths.codex_requirements[0].clone();
    let legacy = requirements.with_file_name("managed_config.toml");
    fx.paths.codex_requirements.push(legacy.clone());
    (fx, requirements, legacy)
}

fn managed_files(loaded: &Loaded) -> Vec<PathBuf> {
    let managed = loaded
        .rules
        .iter()
        .filter(|rule| rule.source == Source::CodexManaged);
    managed.map(|rule| rule.file.clone()).collect()
}

fn blocked_by(loaded: &Loaded, file: &Path) -> bool {
    let file = file.display().to_string();
    loaded.blocked().is_some_and(|reason| {
        reason.starts_with("Codex rules cannot be read") && reason.contains(&file)
    })
}

#[test]
fn missing_requirements_and_legacy_files_load_nothing_and_block_nothing() {
    let (fx, _, _) = with_legacy();
    let loaded = fx.load(true);
    assert_eq!(loaded.blocked(), None);
    assert!(managed_files(&loaded).is_empty());
}

#[test]
fn requirements_that_cannot_be_read_block_and_the_legacy_file_is_no_fallback() {
    let (fx, requirements, legacy) = with_legacy();
    write(&legacy, RM);
    std::fs::create_dir_all(&requirements).unwrap();
    let folder = fx.load(true);
    assert!(blocked_by(&folder, &requirements), "{:?}", folder.blocked());
    assert!(
        managed_files(&folder).is_empty(),
        "no rules from either file"
    );
    std::fs::remove_dir(&requirements).unwrap();
    std::fs::write(&requirements, b"[rules]\n\xff\xfe").unwrap();
    let utf8 = fx.load(true);
    assert!(blocked_by(&utf8, &requirements), "{:?}", utf8.blocked());
    assert!(managed_files(&utf8).is_empty());
}

#[test]
fn a_toml_or_rule_error_in_requirements_blocks() {
    let fx = fixture();
    let requirements = fx.paths.codex_requirements[0].clone();
    let rule = |body: &str| format!("[rules]\nprefix_rules = [{body}]\n");
    for text in [
        "[rules".to_owned(),
        "[rules]\nprefix_rules = []\n".to_owned(),
        "[rules]\nprefix_rules = \"rm\"\n".to_owned(),
        "rules = 1\n".to_owned(),
        "[rules]\n".to_owned(),
        rule(r#"{ pattern = [{ token = "rm" }], decision = "forbidden", justification = 1 }"#),
        rule(r#"{ pattern = [{ token = "ls" }], decision = "allow" }"#),
        rule(r#"{ pattern = [{ token = "rm" }] }"#),
        rule(r#"{ pattern = [], decision = "forbidden" }"#),
        rule(r#"{ pattern = [{ token = " " }], decision = "forbidden" }"#),
        rule(r#"{ pattern = [{ any_of = ["a", ""] }], decision = "forbidden" }"#),
        rule(r#"{ pattern = [{ token = "a", any_of = ["b"] }], decision = "forbidden" }"#),
        rule(r#"{ pattern = [{ token = "rm" }], decision = "forbidden", justification = " " }"#),
    ] {
        write(&requirements, &text);
        let loaded = fx.load(true);
        assert!(
            blocked_by(&loaded, &requirements),
            "{text}: {:?}",
            loaded.blocked()
        );
        assert!(managed_files(&loaded).is_empty(), "{text}");
        assert!(
            loaded
                .notes
                .iter()
                .any(|note| note.starts_with("cannot read Codex rules")),
            "{text}: {:?}",
            loaded.notes
        );
    }
}

#[test]
fn valid_requirements_still_block_on_a_legacy_file_that_cannot_be_read_or_parsed() {
    let (fx, requirements, legacy) = with_legacy();
    write(&requirements, PUSH);
    write(&legacy, "approval_policy = [");
    let syntax = fx.load(true);
    assert!(blocked_by(&syntax, &legacy), "{:?}", syntax.blocked());
    std::fs::remove_file(&legacy).unwrap();
    std::fs::create_dir_all(&legacy).unwrap();
    let folder = fx.load(true);
    assert!(blocked_by(&folder, &legacy), "{:?}", folder.blocked());
}

#[test]
fn legacy_rules_are_not_used_and_requirements_rules_are() {
    let (fx, requirements, legacy) = with_legacy();
    write(&legacy, &format!("approval_policy = \"never\"\n{RM}"));
    let legacy_only = fx.load(true);
    assert_eq!(legacy_only.blocked(), None, "a valid legacy file");
    assert!(
        managed_files(&legacy_only).is_empty(),
        "its [rules] give no rule"
    );
    let legacy_name = legacy.display().to_string();
    assert!(
        legacy_only
            .notes
            .iter()
            .any(|note| note.starts_with(&legacy_name)
                && note.ends_with("move them to requirements.toml")),
        "the load says the rules moved: {:?}",
        legacy_only.notes
    );
    write(&requirements, PUSH);
    let both = fx.load(true);
    assert_eq!(both.blocked(), None);
    let files = managed_files(&both);
    assert!(
        !files.is_empty() && files.iter().all(|file| *file == requirements),
        "{files:?}"
    );
}

#[test]
fn the_codex_managed_source_off_reads_neither_file() {
    let (fx, requirements, legacy) = with_legacy();
    std::fs::create_dir_all(&requirements).unwrap();
    write(&legacy, "a = [");
    write(
        &global_file(&fx.agent),
        r#"{"sources": {"codex-managed": false}}"#,
    );
    let loaded = fx.load(true);
    assert_eq!(loaded.blocked(), None, "{:?}", loaded.codex_unreadable);
    assert!(managed_files(&loaded).is_empty());
}
