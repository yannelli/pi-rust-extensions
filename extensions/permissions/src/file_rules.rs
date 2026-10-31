//! `Read` and `Edit` rules on paths: Bash file targets, symlinks, `!` carve-outs, and the
//! commands a wildcard allow rule cannot approve.

use std::path::{Path, PathBuf};

use crate::evaluate::{Call, Target};
use crate::files::file_args;
use crate::pattern::{PathContext, normalize, path_matches};
use crate::rules::{Decision, Kind, Rule};
use crate::shell::{Script, Word, strip_wrappers};

const EXEC_WRAPPERS: &[&str] = &["watch", "setsid", "ionice", "flock"];
const FIND_ACTIONS: &[&str] = &[
    "-exec",
    "-execdir",
    "-ok",
    "-okdir",
    "-delete",
    "-rm",
    "-fprint",
    "-fprint0",
    "-fprintf",
    "-fls",
    "-files0-from",
];

/// The files a script names through redirections and recognized file commands. A relative
/// path resolves against the cwd and against every `cd` target in the script.
pub fn bash_files(script: &Script, call: &Call) -> Vec<Target> {
    let mut dirs = vec![call.cwd.to_path_buf()];
    let mut named: Vec<(String, bool)> = script
        .redirects
        .iter()
        .map(|r| (r.path.clone(), r.write))
        .collect();
    for words in &script.commands {
        let words = strip_wrappers(words, true);
        if words.first().is_some_and(|w| w.value == "cd") {
            let dir = words.get(1).map_or("~", |w| w.value.as_str());
            dirs.push(normalize(&call.cwd.join(expand_home(dir, call.home))));
        }
        named.extend(file_args(words).into_iter().map(|a| (a.path, a.write)));
    }
    let mut out: Vec<Target> = Vec::new();
    for (path, write) in named {
        let plain = !path.starts_with('~') && !path.contains(['*', '?', '[', '$']);
        let expanded = expand_home(&path, call.home);
        let bases: Vec<&Path> = if expanded.is_absolute() {
            vec![Path::new("/")]
        } else {
            dirs.iter().map(PathBuf::as_path).collect()
        };
        for base in bases {
            let path = normalize(&base.join(&expanded));
            let target = Target::File { path, write, plain };
            if !out.contains(&target) {
                out.push(target);
            }
        }
    }
    out
}

fn expand_home(path: &str, home: &Path) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if path == "~" => home.to_path_buf(),
        None => PathBuf::from(path),
    }
}

/// The real path, resolving symlinks in the existing part of `path`.
pub fn resolve(path: &Path) -> PathBuf {
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => resolve(parent).join(name),
        _ => path.to_path_buf(),
    }
}

/// A path rule against a requested path. Deny and ask rules match the requested path or the
/// file it resolves to; allow rules need both.
pub fn path_rule_matches(spec: &str, cx: &PathContext, path: &Path, decision: Decision) -> bool {
    if spec.starts_with('!') {
        return false;
    }
    let restrict = decision != Decision::Allow;
    let requested = path_matches(spec, cx, path, restrict);
    let real = resolve(path);
    let real_anchor = resolve(cx.anchor);
    let (real_cwd, real_home) = (resolve(cx.cwd), resolve(cx.home));
    let real_cx = PathContext {
        cwd: &real_cwd,
        home: &real_home,
        anchor: &real_anchor,
    };
    let real_spec = real_pattern(spec, cx);
    let resolved = path_matches(spec, cx, &real, restrict)
        || path_matches(spec, &real_cx, &real, restrict)
        || real_spec.is_some_and(|p| path_matches(&p, cx, &real, restrict));
    if restrict {
        requested || resolved
    } else {
        requested && resolved
    }
}

/// An anchored pattern rewritten at the real location of its literal directory prefix.
fn real_pattern(spec: &str, cx: &PathContext) -> Option<String> {
    let (base, rest) = if let Some(rest) = spec.strip_prefix("//") {
        (PathBuf::from("/"), rest)
    } else if let Some(rest) = spec.strip_prefix("~/") {
        (cx.home.to_path_buf(), rest)
    } else if let Some(rest) = spec.strip_prefix('/') {
        (cx.anchor.to_path_buf(), rest)
    } else {
        return None;
    };
    let segments: Vec<&str> = rest.split('/').collect();
    let literal = segments
        .iter()
        .take_while(|s| !s.contains(['*', '?', '[']))
        .count();
    let literal = literal.min(segments.len().saturating_sub(1));
    let prefix = base.join(segments[..literal].join("/"));
    let real = resolve(&prefix);
    let tail = segments[literal..].join("/");
    (real != prefix).then(|| format!("/{}/{tail}", real.display()))
}

/// True when a later `!` rule of the same file and list carves `target` out of `rule`.
/// A carve-out cannot reopen a file inside a directory that `rule` blocks as a whole.
pub fn carved(rules: &[Rule], rule: &Rule, call: &Call, target: &Target) -> bool {
    let (Kind::Claude(claude), Some(path)) = (&rule.kind, target_path(target)) else {
        return false;
    };
    let Some(spec) = claude.spec.as_deref() else {
        return false;
    };
    let relative = !spec.starts_with(['/', '~', '!']);
    if rule.decision == Decision::Allow
        || !relative
        || !matches!(claude.tool.as_str(), "Read" | "Edit")
    {
        return false;
    }
    let cx = PathContext {
        cwd: call.cwd,
        home: call.home,
        anchor: &claude.anchor,
    };
    let Some(index) = rules.iter().position(|r| std::ptr::eq(r, rule)) else {
        return false;
    };
    let carve = rules[index + 1..].iter().any(|later| {
        let Kind::Claude(other) = &later.kind else {
            return false;
        };
        let negated = other.spec.as_deref().and_then(|s| s.strip_prefix('!'));
        later.file == rule.file
            && later.decision == rule.decision
            && other.tool == claude.tool
            && negated
                .is_some_and(|n| !n.starts_with(['/', '~']) && path_matches(n, &cx, path, true))
    });
    let blocks_parent = path
        .ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(call.cwd) && *dir != call.cwd)
        .any(|dir| path_matches(spec, &cx, dir, true));
    carve && !blocks_parent
}

fn target_path(target: &Target) -> Option<&Path> {
    match target {
        Target::Path(path) | Target::File { path, .. } => Some(path),
        _ => None,
    }
}

/// Commands a wildcard allow rule cannot approve: exec wrappers and `find` actions that run,
/// delete, or write.
pub fn needs_exact_rule(words: &[Word]) -> bool {
    let Some(first) = words.first() else {
        return false;
    };
    let name = first.value.rsplit('/').next().unwrap_or_default();
    EXEC_WRAPPERS.contains(&name)
        || (name == "find"
            && words
                .iter()
                .any(|w| FIND_ACTIONS.contains(&w.value.as_str())))
}

/// A Bash file target inside a working directory that needs no allow rule of its own.
pub fn inside_dirs(target: &Target, call: &Call) -> bool {
    match target {
        Target::File {
            path, plain: true, ..
        } => {
            let real = resolve(path);
            call.dirs
                .iter()
                .any(|dir| path.starts_with(dir) && real.starts_with(resolve(dir)))
        }
        _ => false,
    }
}
