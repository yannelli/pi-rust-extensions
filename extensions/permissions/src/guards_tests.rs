//! Writes to Claude protected paths through the loaded settings: each mode's outcome, allow rules that do not
//! pre-approve them, deny rules that still win, Bash write targets, symlinks, and the `/permissions` sample test.

use serde_json::{Value, json};

use super::tests::{fixture, write};
use super::*;
use crate::evaluate::{Call, evaluate};
use crate::guards::{Guard, guard, protected_name};

/// User Claude settings with `defaultMode` `mode` and `allow` rules; `None` turns the mode off for pi.
fn loaded(fx: &super::tests::Fixture, mode: Option<&str>, allow: &[&str], deny: &[&str]) -> Loaded {
    let mut permissions = json!({ "allow": allow, "deny": deny });
    match mode {
        Some(mode) => permissions["defaultMode"] = json!(mode),
        None => write(&global_file(&fx.agent), r#"{"defaultMode": "pi"}"#),
    }
    let settings = json!({ "permissions": permissions }).to_string();
    write(&fx.paths.claude_dir.join("settings.json"), &settings);
    fx.load(true)
}

fn decide(loaded: &Loaded, tool: &str, input: Value) -> Option<Guard> {
    let call = Call {
        tool,
        input: &input,
        cwd: &loaded.cwd,
        home: &loaded.cwd,
        dirs: &loaded.dirs,
    };
    guard(loaded, &call, &evaluate(&loaded.rules, &call))
}

fn edit(loaded: &Loaded, path: &str) -> Option<Guard> {
    decide(
        loaded,
        "edit",
        json!({ "path": path, "oldText": "a", "newText": "b" }),
    )
}

#[test]
fn protected_names_match_whole_components_and_basenames() {
    let yes = [
        ".git/config",
        "a/b/.git/hooks/pre-commit",
        ".git",
        ".config/git/config",
        ".claude/settings.json",
        "home/.claude/agents/x.md",
        ".vscode/settings.json",
        "pkg/.cargo/config.toml",
        "home/.zshrc",
        "web/.npmrc",
        ".mcp.json",
        "home/.claude.json",
        "gradle/wrapper/gradle-wrapper.properties",
        ".claude/worktrees/w/.git/config",
        ".claude/worktrees/w/.claude/settings.json",
        ".claude/worktrees",
        ".claude/worktreesx/a",
    ];
    let no = [
        ".claude/worktrees/w/src/a.rs",
        "src/git.rs",
        ".gitignore",
        "src/git/x",
        ".config/gitx/a",
        "notes/.zshrc.bak",
        "mcp.json",
        ".GIT/config",
        ".Claude/settings.json",
    ];
    for path in yes {
        assert!(
            protected_name(Path::new(path)),
            "{path} should be protected"
        );
    }
    for path in no {
        assert!(
            !protected_name(Path::new(path)),
            "{path} should not be protected"
        );
    }
}

#[test]
fn each_mode_handles_a_protected_edit_and_an_allow_rule_does_not_pre_approve_it() {
    let allow = ["Edit(./.claude/**)", "Edit(./.git/**)"];
    let fx = fixture();
    let accept = loaded(&fx, Some("acceptEdits"), &allow, &[]);
    assert!(
        matches!(edit(&accept, ".claude/settings.json"), Some(Guard::Ask(r)) if r.contains("protected path"))
    );
    assert_eq!(
        edit(&accept, "src/a.rs"),
        None,
        "an ordinary edit is left to the mode"
    );
    assert!(matches!(
        edit(&loaded(&fx, Some("default"), &allow, &[]), ".git/config"),
        Some(Guard::Ask(_))
    ));
    assert!(matches!(
        edit(&loaded(&fx, Some("auto"), &allow, &[]), ".git/config"),
        Some(Guard::Ask(_))
    ));
    let dont_ask = edit(&loaded(&fx, Some("dontAsk"), &allow, &[]), ".git/config");
    assert!(matches!(dont_ask, Some(Guard::Deny(r)) if r.contains("dontAsk")));
    let plan = edit(&loaded(&fx, Some("plan"), &allow, &[]), ".git/config");
    assert!(matches!(plan, Some(Guard::Deny(r)) if r.contains("plan mode")));
    assert_eq!(
        edit(
            &loaded(&fx, Some("bypassPermissions"), &allow, &[]),
            ".git/config"
        ),
        None
    );
    assert_eq!(
        edit(&loaded(&fx, None, &allow, &[]), ".git/config"),
        None,
        "no Claude mode, no guard"
    );
}

#[test]
fn bash_write_targets_are_checked_and_reads_are_not() {
    let fx = fixture();
    let accept = loaded(
        &fx,
        Some("acceptEdits"),
        &["Bash(echo *)", "Bash(make *)", "Bash(tee *)"],
        &[],
    );
    let bash = |command: &str| decide(&accept, "bash", json!({ "command": command }));
    assert!(matches!(
        bash("echo x > .claude/settings.json"),
        Some(Guard::Ask(_))
    ));
    assert!(
        matches!(
            bash("make | tee build.log .gitmodules"),
            Some(Guard::Ask(_))
        ),
        "the second tee file"
    );
    assert_eq!(bash("cat .git/config"), None);
    assert_eq!(bash("echo x > build.log"), None);
}

#[test]
fn a_deny_rule_still_decides_and_the_sample_test_shows_the_same_guard() {
    let fx = fixture();
    let denied = loaded(&fx, Some("acceptEdits"), &[], &["Edit(./.git/**)"]);
    let input = json!({ "path": ".git/config", "oldText": "a", "newText": "b" });
    let call = Call {
        tool: "edit",
        input: &input,
        cwd: &denied.cwd,
        home: &denied.cwd,
        dirs: &denied.dirs,
    };
    assert_eq!(
        evaluate(&denied.rules, &call).decision,
        Some(Decision::Deny)
    );
    assert!(crate::listing::explain(&denied, &call)[0].starts_with("edit: deny"));
    let accept = loaded(&fx, Some("acceptEdits"), &["Edit(./.git/**)"], &[]);
    let call = Call {
        tool: "edit",
        input: &input,
        cwd: &accept.cwd,
        home: &accept.cwd,
        dirs: &accept.dirs,
    };
    let expected = guard(&accept, &call, &evaluate(&accept.rules, &call))
        .unwrap()
        .describe();
    assert_eq!(
        crate::listing::explain(&accept, &call)[0],
        format!("edit: {expected}")
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_into_a_protected_path_and_a_linked_settings_file_target_are_protected() {
    let fx = fixture();
    std::fs::create_dir_all(fx.cwd.join(".git")).unwrap();
    std::os::unix::fs::symlink(fx.cwd.join(".git"), fx.cwd.join("link")).unwrap();
    let dotfiles = fx.paths.home.join("dotfiles/claude-settings.json");
    write(
        &dotfiles,
        r#"{"permissions": {"defaultMode": "acceptEdits"}}"#,
    );
    std::fs::create_dir_all(&fx.paths.claude_dir).unwrap();
    std::os::unix::fs::symlink(&dotfiles, fx.paths.claude_dir.join("settings.json")).unwrap();
    let accept = fx.load(true);
    assert!(
        accept
            .protected_files
            .contains(&crate::file_rules::resolve(&dotfiles)),
        "{:?}",
        accept.protected_files
    );
    assert!(
        matches!(edit(&accept, "link/config"), Some(Guard::Ask(_))),
        "resolves into .git"
    );
    assert!(
        matches!(
            edit(&accept, &dotfiles.display().to_string()),
            Some(Guard::Ask(_))
        ),
        "the linked file"
    );
    assert_eq!(edit(&accept, "src/a.rs"), None);
}
