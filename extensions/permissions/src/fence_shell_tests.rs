//! Bash under the read block: directory changes the target finder cannot follow and commands handed to a
//! shell or another runner ask in `bypassPermissions` and deny in `dontAsk`; traceable reads stay as they were.

use serde_json::json;

use super::tests::{Fixture, fixture, write};
use super::*;
use crate::evaluate::{Call, evaluate};
use crate::fence::fence;
use crate::guards::Guard;

const UNRESOLVED_CDS: &[&str] = &[
    "cd $HOME && cat .ssh/id_rsa",
    "cd \"$HOME\" && cat .ssh/id_rsa",
    "cd - && cat a.txt",
    "cd ~other && cat a.txt",
    "cd -P sub && cat a.txt",
    "cd -- sub && cat a.txt",
    "cd sub other && cat a.txt",
    "cd s* && cat a.txt",
    "cd {a,b} && cat a.txt",
    "pushd sub && cat a.txt",
    "popd && cat a.txt",
    "builtin pushd sub && cat a.txt",
    "builtin cd sub && builtin cd deeper && cat a.txt",
    "cd sub; command cd deeper; cat a.txt",
];

const NESTED: &[&str] = &[
    "sh -c 'cat a.txt'",
    "/bin/sh -c 'cat a.txt'",
    "bash x.sh",
    "/usr/local/bin/zsh -c 'cat a.txt'",
    "echo a | sh",
    "eval 'cat a.txt'",
    "builtin eval 'cat a.txt'",
    "source x.sh",
    ". x.sh",
    "xargs cat < list.txt",
    "xargs -0 cat < list.txt",
    "timeout 5 bash -c 'cat a.txt'",
    "nohup /bin/dash x.sh",
    "command -p sh -c 'cat a.txt'",
    "/usr/bin/env cat a.txt",
    "exec cat a.txt",
    "sudo cat a.txt",
    "find . -exec cat {} +",
];

fn block(fx: &Fixture, mode: &str) -> Loaded {
    let settings = json!({ "permissions": { "blockReadsOutsideWorkingDirectories": true, "defaultMode": mode } });
    write(
        &fx.paths.claude_dir.join("settings.json"),
        &settings.to_string(),
    );
    fx.load(true)
}

fn bash(fx: &Fixture, loaded: &Loaded, command: &str) -> Option<Guard> {
    let input = json!({ "command": command });
    let call = Call {
        tool: "bash",
        input: &input,
        cwd: &loaded.cwd,
        home: &fx.paths.home,
        dirs: &loaded.dirs,
    };
    fence(loaded, &call, &evaluate(&loaded.rules, &call))
}

fn untraced_ask(guard: &Option<Guard>) -> bool {
    matches!(guard, Some(Guard::Ask(reason)) if reason.contains("cannot be traced"))
}

fn dont_ask_deny(guard: &Option<Guard>) -> bool {
    matches!(guard, Some(Guard::Deny(reason)) if reason.contains("blockReadsOutsideWorkingDirectories") && reason.contains("dontAsk"))
}

#[test]
fn directory_changes_that_cannot_be_followed_ask_in_bypass_and_deny_in_dont_ask() {
    let fx = fixture();
    let bypass = block(&fx, "bypassPermissions");
    for script in UNRESOLVED_CDS {
        let guard = bash(&fx, &bypass, script);
        assert!(untraced_ask(&guard), "{script}: {guard:?}");
    }
    let dont_ask = block(&fx, "dontAsk");
    for script in UNRESOLVED_CDS {
        let guard = bash(&fx, &dont_ask, script);
        assert!(dont_ask_deny(&guard), "{script}: {guard:?}");
    }
}

#[test]
fn nested_shells_eval_source_xargs_and_runners_ask_in_bypass_and_deny_in_dont_ask() {
    let fx = fixture();
    let bypass = block(&fx, "bypassPermissions");
    for script in NESTED {
        let guard = bash(&fx, &bypass, script);
        assert!(untraced_ask(&guard), "{script}: {guard:?}");
    }
    let dont_ask = block(&fx, "dontAsk");
    for script in NESTED {
        let guard = bash(&fx, &dont_ask, script);
        assert!(dont_ask_deny(&guard), "{script}: {guard:?}");
    }
}

#[test]
fn traceable_directory_changes_reads_and_redirects_keep_their_result() {
    let fx = fixture();
    let loaded = block(&fx, "bypassPermissions");
    for script in [
        "cat ./a.txt",
        "cd sub && cat a.txt",
        "builtin cd sub && cat a.txt",
        "command cat a.txt",
        "timeout 5 cat a.txt",
        "echo x > out.txt",
        "cd ./sub && echo x > out.txt",
    ] {
        assert_eq!(bash(&fx, &loaded, script), None, "{script}");
    }
    let home = format!("cd {} && cat a.txt", fx.paths.home.display());
    for script in [
        "cd && cat a.txt",
        "cd ~ && cat a.txt",
        "cd ~/x && cat a.txt",
        home.as_str(),
    ] {
        let guard = bash(&fx, &loaded, script);
        assert!(
            matches!(&guard, Some(Guard::Ask(reason)) if reason.contains("outside the working directories")),
            "{script}: the home directory is followed and outside: {guard:?}"
        );
    }
}
