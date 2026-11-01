//! Claude protected paths: writes that no mode auto-approves except `bypassPermissions`, and that an
//! allow rule does not pre-approve (Claude Code permission-modes "Protected paths").

use std::path::{Component, Path};

use crate::evaluate::{Call, Target, Verdict};
use crate::file_rules::resolve;
use crate::load::Loaded;
use crate::mode::Mode;

const DIRS: &[&str] = &[
    ".git",
    ".vscode",
    ".idea",
    ".husky",
    ".cargo",
    ".devcontainer",
    ".yarn",
    ".mvn",
    ".claude",
];
const FILES: &[&str] = &[
    ".gitconfig",
    ".gitmodules",
    ".bashrc",
    ".bash_profile",
    ".bash_login",
    ".bash_aliases",
    ".bash_logout",
    ".zshrc",
    ".zprofile",
    ".zshenv",
    ".zlogin",
    ".zlogout",
    ".profile",
    ".envrc",
    ".npmrc",
    ".yarnrc",
    ".yarnrc.yml",
    ".pnp.cjs",
    ".pnp.loader.mjs",
    ".pnpmfile.cjs",
    "bunfig.toml",
    ".bunfig.toml",
    ".bazelrc",
    ".bazelversion",
    ".bazeliskrc",
    ".pre-commit-config.yaml",
    "lefthook.yml",
    "lefthook.yaml",
    ".lefthook.yml",
    ".lefthook.yaml",
    "gradle-wrapper.properties",
    "maven-wrapper.properties",
    ".devcontainer.json",
    ".ripgreprc",
    "pyrightconfig.json",
    ".mcp.json",
    ".claude.json",
];

/// A protected write: an ask (with the reason the dialog shows) or a deny reason.
#[derive(Debug, Clone, PartialEq)]
pub enum Guard {
    Ask(String),
    Deny(String),
}

impl Guard {
    pub fn describe(&self) -> String {
        match self {
            Guard::Ask(reason) => format!("ask ({reason})"),
            Guard::Deny(reason) => format!("deny ({reason})"),
        }
    }
}

/// Whether `path` is in a protected directory or names a protected file, by exact (case-sensitive) names.
/// A `.claude` directly above `worktrees/<name>` is not protected, but anything protected below it is.
pub fn protected_name(path: &Path) -> bool {
    let parts: Vec<&str> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part.to_str().unwrap_or_default()),
            _ => None,
        })
        .collect();
    let worktree = |i: usize| parts.get(i + 1) == Some(&"worktrees") && parts.len() > i + 2;
    let dir = parts.iter().enumerate().any(|(i, part)| {
        (DIRS.contains(part) && !(*part == ".claude" && worktree(i)))
            || (*part == ".config" && parts.get(i + 1) == Some(&"git"))
    });
    dir || parts.last().is_some_and(|name| FILES.contains(name))
}

/// Whether a write to `path` is protected: by its requested path, the file it resolves to, or as the
/// target of a Claude settings file that is a symlink.
pub fn protected(path: &Path, loaded: &Loaded) -> bool {
    let real = resolve(path);
    [path, real.as_path()]
        .into_iter()
        .any(|p| protected_name(p) || loaded.protected_files.iter().any(|f| f == p))
}

/// The protected write of `call`, while a Claude mode applies; `None` in `bypassPermissions` and
/// without a mode. pi's `plan` denies writes, where Claude can prompt or bypass in plan.
pub fn guard(loaded: &Loaded, call: &Call, verdict: &Verdict) -> Option<Guard> {
    let (mode, _) = loaded.mode.as_ref()?;
    let mut written = verdict
        .targets
        .iter()
        .filter_map(|(target, _)| match target {
            Target::Path(path) if matches!(call.tool, "edit" | "write") => Some(path),
            Target::File {
                path, write: true, ..
            } => Some(path),
            _ => None,
        });
    let path = written.find(|path| protected(path, loaded))?;
    let reason = format!(
        "{} writes {}, a Claude protected path, which no allow rule pre-approves",
        call.tool,
        path.display()
    );
    match mode {
        Mode::Bypass => None,
        Mode::Default | Mode::AcceptEdits => Some(Guard::Ask(reason)),
        Mode::DontAsk => Some(Guard::Deny(format!(
            "{reason}. Claude dontAsk mode denies calls that would ask."
        ))),
        Mode::Plan => Some(Guard::Deny(format!(
            "{reason}. Claude plan mode does not edit files."
        ))),
    }
}
