//! Claude `defaultMode` for the calls that no allow, ask, or deny rule decides.

use crate::evaluate::{Call, Target, Verdict};
use crate::file_rules::{needs_exact_rule, within};
use crate::rules::{Decision, Source};
use crate::shell::{Word, strip_wrappers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Default,
    AcceptEdits,
    Plan,
    DontAsk,
    Bypass,
}

impl Mode {
    /// `manual` is Claude's alias of `default`. pi has no classifier, so `auto` asks like
    /// `default`.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "default" | "manual" | "auto" => Mode::Default,
            "acceptEdits" => Mode::AcceptEdits,
            "plan" => Mode::Plan,
            "dontAsk" => Mode::DontAsk,
            "bypassPermissions" => Mode::Bypass,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Run,
    Ask,
    Deny(String),
}

/// The mode that applies, with its label: managed Claude settings first, then the pi
/// `defaultMode` setting (`claude` takes the Claude settings, a name sets the mode).
pub fn effective(pi: Option<&str>, claude: Option<&(String, Source)>) -> Option<(Mode, String)> {
    if let Some((name, Source::ClaudeManaged)) = claude {
        return Mode::parse(name).map(|m| (m, format!("{name} (claude-managed)")));
    }
    match pi {
        Some("pi") => None,
        None | Some("claude") => {
            let (name, source) = claude?;
            Mode::parse(name).map(|m| (m, format!("{name} ({})", source.label())))
        }
        Some(name) => Mode::parse(name).map(|m| (m, format!("{name} (pi)"))),
    }
}

/// What `mode` does with a call that no rule decided.
pub fn decide(mode: Mode, call: &Call, verdict: &Verdict) -> Outcome {
    if mode == Mode::Bypass || !needs_approval(mode, call, verdict) {
        return Outcome::Run;
    }
    let writes = matches!(call.tool, "edit" | "write")
        || verdict
            .targets
            .iter()
            .any(|(t, _)| matches!(t, Target::File { write: true, .. }));
    match mode {
        Mode::Plan if writes => Outcome::Deny("Claude plan mode does not edit files.".to_owned()),
        Mode::DontAsk => {
            Outcome::Deny("Claude dontAsk mode denies calls that would ask.".to_owned())
        }
        _ => Outcome::Ask,
    }
}

fn needs_approval(mode: Mode, call: &Call, verdict: &Verdict) -> bool {
    let inside = |path: &std::path::Path| within(path, call.dirs);
    let edits_free = mode == Mode::AcceptEdits;
    match call.tool {
        "edit" | "write" => {
            !edits_free
                || verdict
                    .targets
                    .iter()
                    .any(|(t, _)| !matches!(t, Target::Path(p) if inside(p)))
        }
        "read" | "grep" | "find" | "ls" => verdict
            .targets
            .iter()
            .any(|(t, _)| matches!(t, Target::Path(p) if !inside(p))),
        "bash" => verdict.targets.iter().any(|(target, rule)| {
            let allowed = rule.as_ref().is_some_and(|r| r.decision == Decision::Allow);
            !allowed
                && match target {
                    Target::Command(words) => {
                        let words = strip_wrappers(words, false);
                        !read_only(words) && !(edits_free && file_command(words))
                    }
                    Target::File { path, write, plain } => {
                        !plain || !inside(path) || (*write && !edits_free)
                    }
                    Target::Shell(_) => false,
                    _ => true,
                }
        }),
        "fetch" | "web_fetch" | "url_context" | "web_search" => true,
        tool => tool.starts_with("mcp__"),
    }
}

/// Claude's built-in read-only commands, which run without a prompt in every mode.
fn read_only(words: &[Word]) -> bool {
    let Some(first) = words.first() else {
        return true;
    };
    let args: Vec<&str> = words[1..].iter().map(|w| w.value.as_str()).collect();
    match first.value.as_str() {
        "ls" | "cat" | "echo" | "pwd" | "head" | "tail" | "grep" | "wc" | "which" | "diff"
        | "stat" | "du" | "cd" | "true" | "false" => true,
        "find" => !needs_exact_rule(words),
        "git" => match args.first().copied() {
            Some("status" | "log" | "diff" | "show" | "rev-parse" | "ls-files" | "blame")
            | Some("describe" | "shortlog") => true,
            Some("branch") => args[1..]
                .iter()
                .all(|a| matches!(*a, "-a" | "-r" | "-v" | "--list")),
            Some("remote") => args[1..].iter().all(|a| *a == "-v"),
            _ => false,
        },
        _ => false,
    }
}

/// `mkdir`, `touch`, `mv`, and `cp`, which `acceptEdits` runs inside the working directories.
fn file_command(words: &[Word]) -> bool {
    words
        .first()
        .is_some_and(|w| matches!(w.value.as_str(), "mkdir" | "touch" | "mv" | "cp"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluate::evaluate;
    use serde_json::{Value, json};
    use std::path::{Path, PathBuf};

    fn outcome(mode: Mode, tool: &str, input: Value) -> Outcome {
        let dirs = [PathBuf::from("/w"), PathBuf::from("/extra")];
        let call = Call {
            tool,
            input: &input,
            cwd: Path::new("/w"),
            home: Path::new("/h"),
            dirs: &dirs,
        };
        decide(mode, &call, &evaluate(&[], &call))
    }

    fn bash(mode: Mode, command: &str) -> Outcome {
        outcome(mode, "bash", json!({ "command": command }))
    }

    #[test]
    fn modes_resolve_with_managed_first() {
        let user = ("plan".to_owned(), Source::ClaudeUser);
        let managed = ("dontAsk".to_owned(), Source::ClaudeManaged);
        assert_eq!(effective(None, Some(&user)).unwrap().0, Mode::Plan);
        assert_eq!(effective(Some("pi"), Some(&user)), None);
        assert_eq!(
            effective(Some("claude"), Some(&user)).unwrap().0,
            Mode::Plan
        );
        assert_eq!(
            effective(Some("acceptEdits"), None).unwrap().0,
            Mode::AcceptEdits
        );
        assert_eq!(effective(None, Some(&managed)).unwrap().0, Mode::DontAsk);
        assert_eq!(
            effective(Some("bypassPermissions"), Some(&managed))
                .unwrap()
                .0,
            Mode::DontAsk
        );
        assert_eq!(Mode::parse("auto"), Some(Mode::Default));
    }

    #[test]
    fn default_mode_asks_for_changes_and_reads_outside() {
        assert_eq!(bash(Mode::Default, "ls -la && git status"), Outcome::Run);
        assert_eq!(bash(Mode::Default, "npm test"), Outcome::Ask);
        assert_eq!(bash(Mode::Default, "cat notes.txt > out.txt"), Outcome::Ask);
        assert_eq!(bash(Mode::Default, "cat /etc/hosts"), Outcome::Ask);
        assert_eq!(bash(Mode::Default, "find . -delete"), Outcome::Ask);
        let read = |path: &str| outcome(Mode::Default, "read", json!({ "path": path }));
        assert_eq!(read("src/a.rs"), Outcome::Run);
        assert_eq!(read("/extra/b.rs"), Outcome::Run);
        assert_eq!(read("/etc/passwd"), Outcome::Ask);
        assert_eq!(
            outcome(Mode::Default, "write", json!({ "path": "a" })),
            Outcome::Ask
        );
        assert_eq!(
            outcome(Mode::Default, "mcp__fs__x", json!({})),
            Outcome::Ask
        );
        assert_eq!(outcome(Mode::Default, "todo", json!({})), Outcome::Run);
    }

    #[test]
    fn accept_edits_plan_dont_ask_and_bypass() {
        let write = |mode, path: &str| outcome(mode, "write", json!({ "path": path }));
        assert_eq!(write(Mode::AcceptEdits, "a.txt"), Outcome::Run);
        assert_eq!(write(Mode::AcceptEdits, "/etc/x"), Outcome::Ask);
        assert_eq!(
            bash(Mode::AcceptEdits, "mkdir out && cp a out/b"),
            Outcome::Run
        );
        assert!(matches!(write(Mode::Plan, "a.txt"), Outcome::Deny(_)));
        assert_eq!(bash(Mode::Plan, "npm test"), Outcome::Ask);
        assert!(matches!(
            bash(Mode::Plan, "echo x > notes.txt"),
            Outcome::Deny(_)
        ));
        assert!(matches!(
            bash(Mode::Plan, "make | tee build.log"),
            Outcome::Deny(_)
        ));
        assert!(matches!(
            bash(Mode::Plan, "sed -i s/a/b/ f.txt"),
            Outcome::Deny(_)
        ));
        assert_eq!(bash(Mode::Plan, "cat a.txt"), Outcome::Run);
        assert!(matches!(bash(Mode::DontAsk, "npm test"), Outcome::Deny(_)));
        assert_eq!(bash(Mode::DontAsk, "git log"), Outcome::Run);
        assert_eq!(bash(Mode::Bypass, "rm -rf build"), Outcome::Run);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_working_dirs_is_outside() {
        let root = tempfile::tempdir().unwrap();
        let (work, outside) = (root.path().join("work"), root.path().join("outside"));
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("x.txt"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, work.join("link")).unwrap();
        let dirs = [work.clone()];
        let run = |mode, tool: &str, input: Value| {
            let call = Call {
                tool,
                input: &input,
                cwd: &work,
                home: Path::new("/h"),
                dirs: &dirs,
            };
            decide(mode, &call, &evaluate(&[], &call))
        };
        let write = json!({ "path": "link/out.txt" });
        assert_eq!(run(Mode::AcceptEdits, "write", write), Outcome::Ask);
        assert_eq!(
            run(Mode::AcceptEdits, "write", json!({ "path": "own.txt" })),
            Outcome::Run
        );
        assert_eq!(
            run(Mode::Default, "read", json!({ "path": "link/x.txt" })),
            Outcome::Ask
        );
        let echo = json!({ "command": "echo x > link/out.txt" });
        assert_eq!(run(Mode::AcceptEdits, "bash", echo), Outcome::Ask);
    }
}
