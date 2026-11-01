//! Claude `permissions.blockReadsOutsideWorkingDirectories`: file tools refuse reads outside the working
//! directories in every mode, and Bash reads outside them ask (Claude Code settings-reference).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::evaluate::{Call, Target, Verdict};
use crate::file_rules::{resolve, within};
use crate::guards::Guard;
use crate::load::Loaded;
use crate::mode::Mode;
use crate::shell::{Script, Word, strip_wrappers};

const READ_TOOLS: &[&str] = &["read", "grep", "find", "ls"];
/// Commands that run a script or another command. `strip_wrappers` drops `xargs` and `command` before a
/// command word, so these names match what it keeps.
const DISPATCH: &[&str] = &[
    "sh", "bash", "zsh", "dash", "ksh", "mksh", "ash", "fish", "csh", "tcsh", "eval", "source",
    ".", "xargs", "env", "exec", "command", "sudo", "doas", "watch", "setsid", "ionice", "flock",
];
const FIND_EXEC: &[&str] = &["-exec", "-execdir", "-ok", "-okdir"];
const DIRECTORY_STACK: &[&str] = &["pushd", "popd"];
const GIT_TIMEOUT: Duration = Duration::from_secs(2);
const UNTRACKED: &str = "did not match any file(s) known to git";

/// pi's read-tool skill folders (`<agentDir>/skills`, `~/.agents/skills`, `.agents/skills` up to the git root), minus
/// any that resolves to a parent of the agent or home dir, or, for a project one, outside its own folder.
fn skill_roots(loaded: &Loaded, home: &Path) -> Vec<PathBuf> {
    let secrets = [resolve(&loaded.agent_dir), resolve(home)];
    let holds_secrets = |real: &Path| secrets.iter().any(|secret| secret.starts_with(real));
    let global = [loaded.agent_dir.join("skills"), home.join(".agents/skills")];
    let mut roots: Vec<PathBuf> = global
        .into_iter()
        .filter(|root| root.is_dir() && !holds_secrets(&resolve(root)))
        .collect();
    let git_root = loaded.cwd.ancestors().find(|dir| dir.join(".git").exists());
    for dir in loaded.cwd.ancestors() {
        let root = dir.join(".agents/skills");
        let real = resolve(&root);
        if root.is_dir() && real.starts_with(resolve(dir)) && !holds_secrets(&real) {
            roots.push(root);
        }
        if Some(dir) == git_root {
            break;
        }
    }
    roots
}

/// Whether git reports `<root>/.claude/settings.local.json` as untracked while `.claude` is no symlink. Any
/// other answer, an error, or no answer within two seconds means repository settings.
pub fn local_is_users(root: &Path) -> bool {
    match std::fs::symlink_metadata(root.join(".claude")) {
        Ok(meta) if !meta.file_type().is_symlink() => {}
        _ => return false,
    }
    let child = Command::new("git")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-C",
        ])
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--error-unmatch",
            "--",
            ".claude/settings.local.json",
        ])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else { return false };
    let deadline = Instant::now() + GIT_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    };
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    status.code() == Some(1) && stderr.contains(UNTRACKED)
}

/// Inside the working directories of the block. A path only inside the local settings' directories asks git
/// once per call whether that file is the user's.
fn inside(loaded: &Loaded, path: &Path, local: &mut Option<bool>) -> bool {
    if within(path, &loaded.fence_dirs) {
        return true;
    }
    if !within(path, &loaded.local_dirs) {
        return false;
    }
    *local.get_or_insert_with(|| local_is_users(&loaded.local_root))
}

/// The command name without its directory: `/bin/sh` is `sh`.
fn name(word: &Word) -> &str {
    word.value.rsplit('/').next().unwrap_or_default()
}

/// `cd` with no argument (home) or one directory that `bash_files` resolves: no option, `-`, variable,
/// glob, brace, or `~user`.
fn traceable_cd(args: &[Word]) -> bool {
    match args {
        [] => true,
        [dir] => {
            let dir = dir.value.as_str();
            let tilde = !dir.starts_with('~') || dir == "~" || dir.starts_with("~/");
            tilde && !dir.starts_with('-') && !dir.contains(['$', '*', '?', '[', '{'])
        }
        _ => false,
    }
}

/// A script whose reads cannot be traced: a subshell, an open quote or operator, a directory change
/// `bash_files` cannot follow or more than one, or a command handed to a shell or another runner.
fn untraceable(script: &Script) -> bool {
    let mut cds = 0;
    for words in &script.commands {
        let rest = strip_wrappers(words, true);
        let Some(first) = rest.first() else { continue };
        let stripped = &words[..words.len() - rest.len()];
        let runs_command = DISPATCH.contains(&name(first))
            || stripped.iter().any(|word| name(word) == "xargs")
            || (name(first) == "find"
                && rest.iter().any(|w| FIND_EXEC.contains(&w.value.as_str())));
        if runs_command || DIRECTORY_STACK.contains(&name(first)) {
            return true;
        }
        if first.value == "cd" {
            cds += 1;
            if !traceable_cd(&rest[1..]) {
                return true;
            }
        }
    }
    script.subshell || script.unterminated || !script.complete || cds > 1
}

/// The read fence, then the protected-write guard: what `tool_call` and the `/permissions` test apply after
/// deny rules and before ask rules, allow rules, and the mode.
pub fn check(loaded: &Loaded, call: &Call, verdict: &Verdict) -> Option<Guard> {
    fence(loaded, call, verdict).or_else(|| crate::guards::guard(loaded, call, verdict))
}

/// What the block does with `call` in any mode; `None` while it is off or the call reads only inside.
pub fn fence(loaded: &Loaded, call: &Call, verdict: &Verdict) -> Option<Guard> {
    let source = loaded.block_reads?;
    let label = format!(
        "Claude blockReadsOutsideWorkingDirectories ({})",
        source.label()
    );
    let mut local = None;
    if READ_TOOLS.contains(&call.tool) {
        let skills = skill_roots(loaded, call.home);
        let outside = verdict
            .targets
            .iter()
            .find_map(|(target, _)| match target {
                Target::Path(path)
                    if !within(path, &skills) && !inside(loaded, path, &mut local) =>
                {
                    Some(path)
                }
                _ => None,
            })?;
        return Some(Guard::Deny(format!(
            "{label}: {} is outside the working directories. Add its folder to additionalDirectories in user settings.",
            outside.display()
        )));
    }
    if call.tool != "bash" {
        return None;
    }
    let outside = verdict.targets.iter().find_map(|(target, _)| match target {
        Target::File {
            path,
            write: false,
            plain,
        } if !*plain || !inside(loaded, path, &mut local) => Some(path.display().to_string()),
        _ => None,
    });
    let script = crate::shell::parse(call.input["command"].as_str().unwrap_or_default());
    let why = match (outside, untraceable(&script)) {
        (Some(path), _) => format!("bash reads {path} outside the working directories"),
        (None, true) => "bash runs a script whose reads cannot be traced".to_owned(),
        (None, false) => return None,
    };
    let reason = format!("{label}: {why}");
    Some(match &loaded.mode {
        Some((Mode::DontAsk, _)) => Guard::Deny(format!(
            "{reason}. Claude dontAsk mode denies calls that would ask."
        )),
        _ => Guard::Ask(reason),
    })
}
