use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::args;
use crate::env::Dirs;
use crate::fields::{self, FrontDrops};
use crate::frontmatter::{Doc, substitute};
use crate::plugin::InlineCommand;
use crate::settings::{ArgIndex, Rule};
use crate::source::ensure_inside;

pub const COMMAND_SEPARATOR: &str = if cfg!(windows) { "." } else { ":" };

/// `${NAME}` substitutions.
pub type Vars = Vec<(String, String)>;

/// Where and for whom one plugin is staged.
pub struct Target<'a> {
    pub dirs: &'a Dirs,
    pub plugin: &'a str,
    pub id: String,
    pub root: PathBuf,
    pub data_dir: PathBuf,
    /// `Some(cwd)` for project scope; user scope keeps `${CLAUDE_PROJECT_DIR}`.
    pub project_dir: Option<PathBuf>,
    /// Markdown substitutions after the path variables: non-sensitive user config (as
    /// `user_config.<KEY>` and `CLAUDE_PLUGIN_OPTION_<KEY>`) and the config env.
    pub content_extra: Vars,
    /// MCP server and hook arg substitutions after the path variables: all of them.
    pub config_extra: Vars,
    /// Process env after the path variables: `CLAUDE_PLUGIN_OPTION_<KEY>` and the config env.
    pub env_extra: Vars,
    pub arg_index: ArgIndex,
    pub rules: Vec<Rule>,
}

impl Target<'_> {
    fn path_vars(&self, skill_dir: Option<&Path>) -> Vars {
        let mut vars = vec![
            ("CLAUDE_PLUGIN_ROOT".into(), self.root.display().to_string()),
            (
                "CLAUDE_PLUGIN_DATA".into(),
                self.data_dir.display().to_string(),
            ),
        ];
        if let Some(dir) = skill_dir {
            vars.push(("CLAUDE_SKILL_DIR".into(), dir.display().to_string()));
        }
        if let Some(dir) = &self.project_dir {
            vars.push(("CLAUDE_PROJECT_DIR".into(), dir.display().to_string()));
        }
        vars
    }

    /// Substitutions for prompts, skills, agents, and output styles.
    pub fn vars(&self, skill_dir: Option<&Path>) -> Vars {
        let mut vars = self.path_vars(skill_dir);
        vars.extend(self.content_extra.iter().cloned());
        vars
    }

    /// Substitutions for MCP servers and hook args.
    pub fn config_vars(&self) -> Vars {
        let mut vars = self.path_vars(None);
        vars.extend(self.config_extra.iter().cloned());
        vars
    }

    /// The env of stdio MCP servers and of bash commands that name a plugin bin file.
    pub fn process_env(&self) -> Vars {
        let mut vars = self.path_vars(None);
        vars.extend(self.env_extra.iter().cloned());
        vars
    }

    /// `<plugin>-<name>`, or `name` when it is the plugin name or starts with `<plugin>-`.
    pub fn prefixed(&self, name: &str) -> String {
        if name == self.plugin || name.starts_with(&format!("{}-", self.plugin)) {
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

/// A Claude command as a pi prompt template; reports what pi does not apply.
pub fn convert_command(target: &Target, name: &str, text: &str, drops: &mut FrontDrops) -> String {
    let mut doc = Doc::parse(text);
    drops.add("command", name, &doc.keys());
    if fields::has_shell(&doc.body) {
        drops.feature("command", "shell", name);
    }
    if fields::has_file_ref(&doc.body) {
        drops.feature("command", "file", name);
    }
    let names = doc.get_list("arguments").unwrap_or_default();
    doc.body = args::convert(&doc.body, &names, target.arg_index);
    substitute(&doc.render(), &target.vars(None))
}

fn write_command(
    target: &Target,
    name: &str,
    text: &str,
    files: &mut Vec<PathBuf>,
    drops: &mut FrontDrops,
) -> Result<bool> {
    let dir = target.dirs.prompts();
    let staged = format!("{}{COMMAND_SEPARATOR}{name}", target.plugin);
    let dest = dir.join(format!("{staged}.md"));
    ensure_inside(&dir, &dest)?;
    if files.contains(&dest) {
        return Ok(false);
    }
    std::fs::write(&dest, convert_command(target, &staged, text, drops))?;
    files.push(dest);
    Ok(true)
}

pub fn stage_commands(
    target: &Target,
    paths: &[PathBuf],
    inline: &[InlineCommand],
    files: &mut Vec<PathBuf>,
    drops: &mut FrontDrops,
) -> Result<usize> {
    std::fs::create_dir_all(target.dirs.prompts())?;
    let mut count = 0;
    for path in paths {
        for (parts, file) in markdown_files(path) {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            let name = parts.join(COMMAND_SEPARATOR);
            count += usize::from(write_command(target, &name, &text, files, drops)?);
        }
    }
    for command in inline {
        let text = match (&command.source, &command.content) {
            (Some(file), _) => std::fs::read_to_string(file)
                .with_context(|| format!("cannot read {}", file.display()))?,
            (None, Some(content)) => content.clone(),
            (None, None) => continue,
        };
        let mut doc = Doc::parse(&text);
        let fields = [
            ("description", &command.description),
            ("argument-hint", &command.argument_hint),
            ("model", &command.model),
            ("allowed-tools", &command.allowed_tools),
        ];
        for (key, value) in fields {
            if let Some(value) = value {
                doc.set(key, value);
            }
        }
        let name = command.name.replace(['/', '\\'], COMMAND_SEPARATOR);
        count += usize::from(write_command(target, &name, &doc.render(), files, drops)?);
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

/// A pi skill name: lowercase letters, digits, and `-`.
fn skill_name(name: &str) -> String {
    let name: String = name
        .trim()
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            c @ ('a'..='z' | '0'..='9') => c,
            _ => '-',
        })
        .collect();
    name.trim_matches('-').to_owned()
}

pub fn stage_skills(
    target: &Target,
    paths: &[PathBuf],
    files: &mut Vec<PathBuf>,
    drops: &mut FrontDrops,
) -> Result<usize> {
    let base = target.dirs.skills();
    let mut count = 0;
    for path in paths {
        for skill in skill_dirs(path) {
            let front = std::fs::read_to_string(skill.join("SKILL.md"))
                .map(|text| Doc::parse(&text).get("name"))
                .ok()
                .flatten()
                .map(|name| skill_name(&name))
                .filter(|name| !name.is_empty());
            let dir_name = if skill == target.root {
                Some(target.plugin)
            } else {
                skill.file_name().and_then(|n| n.to_str())
            };
            let Some(name) = front.as_deref().or(dir_name) else {
                continue;
            };
            let staged = target.prefixed(name);
            let dest = base.join(&staged);
            ensure_inside(&base, &dest)?;
            if files.contains(&dest) {
                continue;
            }
            remove_path(&dest)?;
            copy_dir(&skill, &dest)?;
            let skill_md = dest.join("SKILL.md");
            let mut doc = Doc::parse(&std::fs::read_to_string(&skill_md)?);
            drops.add("skill", &staged, &doc.keys());
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
