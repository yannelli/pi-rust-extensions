use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::env::Dirs;
use crate::frontmatter::{Doc, substitute};

pub const COMMAND_SEPARATOR: &str = if cfg!(windows) { "." } else { ":" };

/// Where and for whom one plugin is staged.
pub struct Target<'a> {
    pub dirs: &'a Dirs,
    pub plugin: &'a str,
    pub id: String,
    pub root: PathBuf,
    pub data_dir: PathBuf,
    /// `Some(cwd)` for project scope; user scope keeps `${CLAUDE_PROJECT_DIR}`.
    pub project_dir: Option<PathBuf>,
}

impl Target<'_> {
    pub fn vars(&self, skill_dir: Option<&Path>) -> Vec<(&'static str, String)> {
        let mut vars = vec![
            ("CLAUDE_PLUGIN_ROOT", self.root.display().to_string()),
            ("CLAUDE_PLUGIN_DATA", self.data_dir.display().to_string()),
        ];
        if let Some(dir) = skill_dir {
            vars.push(("CLAUDE_SKILL_DIR", dir.display().to_string()));
        }
        if let Some(dir) = &self.project_dir {
            vars.push(("CLAUDE_PROJECT_DIR", dir.display().to_string()));
        }
        vars
    }

    /// `<plugin>-<name>`, or `name` when it already starts with `<plugin>-`.
    pub fn prefixed(&self, name: &str) -> String {
        if name.starts_with(&format!("{}-", self.plugin)) {
            name.to_owned()
        } else {
            format!("{}-{name}", self.plugin)
        }
    }
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
}

fn is_real(path: &Path, dir: bool) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| if dir { m.is_dir() } else { m.is_file() })
}

fn sorted_children(dir: &Path) -> Vec<PathBuf> {
    let mut children: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| !is_hidden(path))
        .collect();
    children.sort();
    children
}

/// `*.md` files under `path` with their relative name parts (`build/web.md` -> `[build, web]`).
pub fn markdown_files(path: &Path) -> Vec<(Vec<String>, PathBuf)> {
    fn walk(dir: &Path, prefix: &[String], out: &mut Vec<(Vec<String>, PathBuf)>) {
        for child in sorted_children(dir) {
            let Some(name) = child.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let mut parts = prefix.to_vec();
            if is_real(&child, true) {
                parts.push(name.to_owned());
                walk(&child, &parts, out);
            } else if let Some(stem) = name.strip_suffix(".md").filter(|_| is_real(&child, false)) {
                parts.push(stem.to_owned());
                out.push((parts, child));
            }
        }
    }
    let mut out = Vec::new();
    if is_real(path, true) {
        walk(path, &[], &mut out);
    } else if let Some(stem) = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".md"))
    {
        out.push((vec![stem.to_owned()], path.to_path_buf()));
    }
    out
}

pub fn stage_commands(
    target: &Target,
    paths: &[PathBuf],
    files: &mut Vec<PathBuf>,
) -> Result<usize> {
    let dir = target.dirs.prompts();
    std::fs::create_dir_all(&dir)?;
    let vars = target.vars(None);
    let mut count = 0;
    for path in paths {
        for (parts, file) in markdown_files(path) {
            let name = format!(
                "{}{COMMAND_SEPARATOR}{}",
                target.plugin,
                parts.join(COMMAND_SEPARATOR)
            );
            let dest = dir.join(format!("{name}.md"));
            if files.contains(&dest) {
                continue;
            }
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            std::fs::write(&dest, substitute(&text, &vars))?;
            files.push(dest);
            count += 1;
        }
    }
    Ok(count)
}

/// Skill dirs: a path that holds `SKILL.md`, or its subdirs that do.
pub fn skill_dirs(path: &Path) -> Vec<PathBuf> {
    if is_real(&path.join("SKILL.md"), false) {
        return vec![path.to_path_buf()];
    }
    sorted_children(path)
        .into_iter()
        .filter(|child| is_real(child, true) && is_real(&child.join("SKILL.md"), false))
        .collect()
}

fn copy_dir(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    for child in sorted_children(src) {
        let target = dest.join(child.file_name().unwrap_or_default());
        if is_real(&child, true) {
            copy_dir(&child, &target)?;
        } else if is_real(&child, false) {
            std::fs::copy(&child, &target)
                .with_context(|| format!("cannot copy {}", child.display()))?;
        }
    }
    Ok(())
}

pub fn stage_skills(target: &Target, paths: &[PathBuf], files: &mut Vec<PathBuf>) -> Result<usize> {
    let base = target.dirs.skills();
    let mut count = 0;
    for path in paths {
        for skill in skill_dirs(path) {
            let Some(name) = skill.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let staged = target.prefixed(name);
            let dest = base.join(&staged);
            if files.contains(&dest) {
                continue;
            }
            remove_path(&dest)?;
            copy_dir(&skill, &dest)?;
            let skill_md = dest.join("SKILL.md");
            let mut doc = Doc::parse(&std::fs::read_to_string(&skill_md)?);
            doc.set("name", &staged);
            let text = substitute(&doc.render(), &target.vars(Some(&dest)));
            std::fs::write(&skill_md, text)?;
            files.push(dest);
            count += 1;
        }
    }
    Ok(count)
}

pub fn remove_path(path: &Path) -> Result<()> {
    let result = match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(_) => return Ok(()),
    };
    result.with_context(|| format!("cannot remove {}", path.display()))
}
