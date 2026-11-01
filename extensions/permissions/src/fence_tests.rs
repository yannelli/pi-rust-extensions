//! Claude `blockReadsOutsideWorkingDirectories` through the loaded settings: sources and values, which directories
//! count, git's view of the local settings file, modes and allow rules, pi skill folders, and Bash reads.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

use super::tests::{Fixture, fixture, write};
use super::*;
use crate::evaluate::{Call, evaluate};
use crate::fence::{fence, local_is_users};
use crate::guards::Guard;

fn user(fx: &Fixture, permissions: Value) {
    write(
        &fx.paths.claude_dir.join("settings.json"),
        &json!({ "permissions": permissions }).to_string(),
    );
}

fn outside(fx: &Fixture) -> PathBuf {
    let path = fx.paths.home.parent().unwrap().join("outside/a.txt");
    write(&path, "secret");
    path
}

fn decide(fx: &Fixture, loaded: &Loaded, tool: &str, input: Value) -> Option<Guard> {
    let call = Call {
        tool,
        input: &input,
        cwd: &loaded.cwd,
        home: &fx.paths.home,
        dirs: &loaded.dirs,
    };
    fence(loaded, &call, &evaluate(&loaded.rules, &call))
}

fn read(fx: &Fixture, loaded: &Loaded, path: &Path) -> Option<Guard> {
    decide(fx, loaded, "read", json!({ "path": path }))
}

fn bash(fx: &Fixture, loaded: &Loaded, command: &str) -> Option<Guard> {
    decide(fx, loaded, "bash", json!({ "command": command }))
}

fn denied(guard: Option<Guard>) -> bool {
    matches!(guard, Some(Guard::Deny(reason)) if reason.contains("blockReadsOutsideWorkingDirectories"))
}

/// Runs git in `dir`; `false` when git is not installed, so the caller skips.
fn git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .is_ok_and(|out| out.status.success())
}

#[test]
fn true_in_any_loaded_source_turns_the_block_on_and_other_values_do_not() {
    let fx = fixture();
    assert_eq!(fx.load(true).block_reads, None);
    user(&fx, json!({ "blockReadsOutsideWorkingDirectories": false }));
    assert_eq!(fx.load(true).block_reads, None);
    user(&fx, json!({ "blockReadsOutsideWorkingDirectories": "yes" }));
    let loaded = fx.load(true);
    assert_eq!(loaded.block_reads, None);
    assert!(
        loaded
            .notes
            .iter()
            .any(|n| n.contains("must be true or false")),
        "{:?}",
        loaded.notes
    );
    user(&fx, json!({ "blockReadsOutsideWorkingDirectories": true }));
    assert_eq!(fx.load(true).block_reads, Some(Source::ClaudeUser));
    write(
        &global_file(&fx.agent),
        r#"{"sources": {"claude-user": false}}"#,
    );
    assert_eq!(
        fx.load(true).block_reads,
        None,
        "a source that is off sets nothing"
    );
    std::fs::remove_file(global_file(&fx.agent)).unwrap();
    std::fs::remove_file(fx.paths.claude_dir.join("settings.json")).unwrap();
    let project = fx.cwd.join(".claude/settings.json");
    write(
        &project,
        &json!({ "permissions": { "blockReadsOutsideWorkingDirectories": true } }).to_string(),
    );
    assert_eq!(
        fx.load(false).block_reads,
        None,
        "an untrusted project is not read"
    );
    assert_eq!(fx.load(true).block_reads, Some(Source::ClaudeProject));
    write(
        &fx.paths.claude_managed,
        &json!({ "permissions": { "blockReadsOutsideWorkingDirectories": true } }).to_string(),
    );
    assert_eq!(
        fx.load(true).block_reads,
        Some(Source::ClaudeManaged),
        "the first source in load order"
    );
}

#[test]
fn user_and_managed_directories_count_and_project_ones_do_not() {
    let fx = fixture();
    let shared = fx.paths.home.join("shared");
    let project_only = fx.paths.home.join("project-only");
    write(&shared.join("a.txt"), "a");
    write(&project_only.join("a.txt"), "a");
    user(
        &fx,
        json!({ "blockReadsOutsideWorkingDirectories": true, "additionalDirectories": [shared] }),
    );
    let project = json!({ "permissions": { "additionalDirectories": [project_only] } });
    write(&fx.cwd.join(".claude/settings.json"), &project.to_string());
    let loaded = fx.load(true);
    assert_eq!(read(&fx, &loaded, &fx.cwd.join("src/a.rs")), None);
    assert_eq!(read(&fx, &loaded, &shared.join("a.txt")), None);
    assert!(
        denied(read(&fx, &loaded, &project_only.join("a.txt"))),
        "project dirs do not open the fence"
    );
    assert!(denied(read(&fx, &loaded, &outside(&fx))));
    assert!(
        loaded.dirs.contains(&project_only),
        "the project dir is still a working dir for modes"
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_in_the_cwd_that_leaves_the_fence_is_outside() {
    let fx = fixture();
    user(&fx, json!({ "blockReadsOutsideWorkingDirectories": true }));
    let target = outside(&fx);
    std::os::unix::fs::symlink(&target, fx.cwd.join("link.txt")).unwrap();
    assert!(denied(read(&fx, &fx.load(true), &fx.cwd.join("link.txt"))));
}

#[test]
fn local_directories_count_only_while_git_reports_the_local_file_untracked() {
    let fx = fixture();
    std::fs::remove_dir_all(fx.cwd.join(".git")).unwrap();
    if !git(&fx.cwd, &["init", "-q"]) {
        return eprintln!("skipping: git is not installed");
    }
    let shared = fx.paths.home.join("local-shared");
    write(&shared.join("a.txt"), "a");
    user(&fx, json!({ "blockReadsOutsideWorkingDirectories": true }));
    let local = json!({ "permissions": { "additionalDirectories": [shared] } });
    write(
        &fx.cwd.join(".claude/settings.local.json"),
        &local.to_string(),
    );
    assert!(local_is_users(&fx.cwd), "untracked in a repository");
    assert_eq!(read(&fx, &fx.load(true), &shared.join("a.txt")), None);
    assert!(git(&fx.cwd, &["add", ".claude/settings.local.json"]));
    assert!(!local_is_users(&fx.cwd), "tracked");
    assert!(denied(read(&fx, &fx.load(true), &shared.join("a.txt"))));
    std::fs::remove_dir_all(fx.cwd.join(".git")).unwrap();
    assert!(!local_is_users(&fx.cwd), "outside any repository");
    assert!(denied(read(&fx, &fx.load(true), &shared.join("a.txt"))));
}

#[cfg(unix)]
#[test]
fn a_symlinked_claude_folder_makes_local_settings_the_repository_s() {
    let fx = fixture();
    std::fs::remove_dir_all(fx.cwd.join(".git")).unwrap();
    if !git(&fx.cwd, &["init", "-q"]) {
        return eprintln!("skipping: git is not installed");
    }
    let real = fx.paths.home.join("dotclaude");
    write(&real.join("settings.local.json"), "{}");
    std::os::unix::fs::symlink(&real, fx.cwd.join(".claude")).unwrap();
    assert!(!local_is_users(&fx.cwd));
}

#[test]
fn file_tools_are_denied_in_every_mode_and_over_an_allow_rule_while_a_deny_rule_keeps_its_reason() {
    let fx = fixture();
    let target = outside(&fx);
    let allow = format!("Read(/{})", target.display());
    for mode in ["bypassPermissions", "acceptEdits", "dontAsk"] {
        user(
            &fx,
            json!({ "blockReadsOutsideWorkingDirectories": true, "defaultMode": mode, "allow": [allow] }),
        );
        assert!(denied(read(&fx, &fx.load(true), &target)), "{mode}");
    }
    write(&global_file(&fx.agent), r#"{"defaultMode": "pi"}"#);
    assert!(
        denied(read(&fx, &fx.load(true), &target)),
        "with no Claude mode too"
    );
    user(
        &fx,
        json!({ "blockReadsOutsideWorkingDirectories": true, "deny": [allow] }),
    );
    let loaded = fx.load(true);
    let input = json!({ "path": target });
    let call = Call {
        tool: "read",
        input: &input,
        cwd: &loaded.cwd,
        home: &fx.paths.home,
        dirs: &loaded.dirs,
    };
    assert!(
        crate::listing::explain(&loaded, &call)[0].starts_with("read: deny"),
        "the deny rule decides"
    );
}

#[test]
fn pi_skill_folders_stay_readable_and_nothing_else_in_the_agent_dir_does() {
    let fx = fixture();
    user(&fx, json!({ "blockReadsOutsideWorkingDirectories": true }));
    let skill = fx.agent.join("skills/x/SKILL.md");
    let shared = fx.paths.home.join(".agents/skills/y/SKILL.md");
    let auth = fx.agent.join("auth.json");
    for path in [&skill, &shared, &auth] {
        write(path, "x");
    }
    let loaded = fx.load(true);
    assert_eq!(read(&fx, &loaded, &skill), None);
    assert_eq!(read(&fx, &loaded, &shared), None);
    assert!(denied(read(&fx, &loaded, &auth)), "auth.json stays outside");
    let cat = bash(&fx, &loaded, &format!("cat {}", skill.display()));
    assert!(
        matches!(cat, Some(Guard::Ask(_))),
        "Bash gets no skill exemption"
    );
    let edit = json!({ "path": skill, "oldText": "x", "newText": "y" });
    assert_eq!(
        decide(&fx, &loaded, "edit", edit),
        None,
        "edits are not reads"
    );
}

#[test]
fn bash_reads_outside_and_untraceable_scripts_ask_and_dont_ask_denies() {
    let fx = fixture();
    let target = outside(&fx);
    user(
        &fx,
        json!({ "blockReadsOutsideWorkingDirectories": true, "defaultMode": "bypassPermissions" }),
    );
    let loaded = fx.load(true);
    assert!(matches!(
        bash(&fx, &loaded, &format!("cat {}", target.display())),
        Some(Guard::Ask(_))
    ));
    assert!(
        matches!(bash(&fx, &loaded, "cd a && cd b && cat c"), Some(Guard::Ask(r)) if r.contains("cannot be traced"))
    );
    assert!(matches!(bash(&fx, &loaded, "(cat a)"), Some(Guard::Ask(_))));
    assert!(matches!(
        bash(&fx, &loaded, "echo $(cat a)"),
        Some(Guard::Ask(_))
    ));
    assert_eq!(bash(&fx, &loaded, "cat ./a.txt"), None);
    assert_eq!(
        bash(&fx, &loaded, "echo x > out.txt"),
        None,
        "a redirect inside is traceable"
    );
    for script in ["cat a.txt &&", "cat 'a.txt", "cat \"a.txt"] {
        let guard = bash(&fx, &loaded, script);
        assert!(
            matches!(guard, Some(Guard::Ask(_))),
            "{script}: an unfinished script asks"
        );
    }
    user(
        &fx,
        json!({ "blockReadsOutsideWorkingDirectories": true, "defaultMode": "dontAsk" }),
    );
    let dont_ask = bash(&fx, &fx.load(true), &format!("cat {}", target.display()));
    assert!(matches!(dont_ask, Some(Guard::Deny(r)) if r.contains("dontAsk")));
}
