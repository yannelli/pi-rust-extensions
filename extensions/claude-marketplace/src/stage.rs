use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::env::Dirs;
use crate::frontmatter::{Doc, substitute};
use crate::plugin::InlineCommand;
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
    /// `user_config.<KEY>` values for Markdown content: no sensitive values.
    pub content_options: Vars,
    /// Every `user_config.<KEY>` value, for MCP servers and hook args.
    pub config_options: Vars,
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

    /// Path variables and non-sensitive user config, for prompts, skills, agents, and styles.
    pub fn vars(&self, skill_dir: Option<&Path>) -> Vars {
        let mut vars = self.path_vars(skill_dir);
        vars.extend(self.content_options.iter().cloned());
        vars
    }

    /// Path variables and all user config, for MCP servers and hook args.
    pub fn config_vars(&self) -> Vars {
        let mut vars = self.path_vars(None);
        vars.extend(self.config_options.iter().cloned());
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

/// `$ARGUMENTS[N]` (0-based) and the named `$<arg>` of `arguments` become pi's `$<N+1>`.
fn map_arguments(body: &str, names: &[String]) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let word_end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        let word = &after[..word_end];
        let indexed = (word == "ARGUMENTS")
            .then(|| after[word_end..].strip_prefix('['))
            .flatten()
            .and_then(|inner| {
                let close = inner.find(']')?;
                let n: usize = inner[..close].parse().ok()?;
                Some((n, word_end + 2 + close))
            });
        if let Some((n, used)) = indexed {
            out.push_str(&format!("${}", n + 1));
            rest = &after[used..];
        } else if let Some(index) = names.iter().position(|name| name == word) {
            out.push_str(&format!("${}", index + 1));
            rest = &after[word_end..];
        } else {
            out.push('$');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn has_placeholder(body: &str) -> bool {
    body.match_indices('$').any(|(i, _)| {
        let after = &body[i + 1..];
        after.starts_with("ARGUMENTS")
            || after.starts_with('@')
            || after.starts_with("{@")
            || after.starts_with("{ARGUMENTS")
            || after.starts_with(|c: char| c.is_ascii_digit())
            || after
                .strip_prefix('{')
                .is_some_and(|a| a.starts_with(|c: char| c.is_ascii_digit()))
    })
}

/// A Claude command as a pi prompt template. Like Claude Code, a body without placeholders
/// gets the arguments appended.
pub fn convert_command(text: &str, vars: &Vars) -> String {
    let mut doc = Doc::parse(text);
    let names = doc.get_list("arguments").unwrap_or_default();
    doc.body = map_arguments(&doc.body, &names);
    if !has_placeholder(&doc.body) {
        if !doc.body.is_empty() && !doc.body.ends_with('\n') {
            doc.body.push('\n');
        }
        doc.body.push_str("\nARGUMENTS: $ARGUMENTS\n");
    }
    substitute(&doc.render(), vars)
}

fn write_command(
    target: &Target,
    name: &str,
    text: &str,
    files: &mut Vec<PathBuf>,
) -> Result<bool> {
    let dir = target.dirs.prompts();
    let dest = dir.join(format!("{}{COMMAND_SEPARATOR}{name}.md", target.plugin));
    ensure_inside(&dir, &dest)?;
    if files.contains(&dest) {
        return Ok(false);
    }
    std::fs::write(&dest, convert_command(text, &target.vars(None)))?;
    files.push(dest);
    Ok(true)
}

pub fn stage_commands(
    target: &Target,
    paths: &[PathBuf],
    inline: &[InlineCommand],
    files: &mut Vec<PathBuf>,
) -> Result<usize> {
    std::fs::create_dir_all(target.dirs.prompts())?;
    let mut count = 0;
    for path in paths {
        for (parts, file) in markdown_files(path) {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            let name = parts.join(COMMAND_SEPARATOR);
            count += usize::from(write_command(target, &name, &text, files)?);
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
        count += usize::from(write_command(target, &name, &doc.render(), files)?);
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

pub fn stage_skills(target: &Target, paths: &[PathBuf], files: &mut Vec<PathBuf>) -> Result<usize> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_claude_arguments() {
        let names = vec!["file".to_owned(), "mode".to_owned()];
        let body = "A $ARGUMENTS[0] B $ARGUMENTS[1] $file/$mode $files $ARGUMENTS $5 cost $";
        assert_eq!(
            map_arguments(body, &names),
            "A $1 B $2 $1/$2 $files $ARGUMENTS $5 cost $"
        );
    }

    #[test]
    fn appends_arguments_without_placeholders() {
        let vars = Vars::new();
        let text = "---\ndescription: d\n---\nDo it";
        assert_eq!(
            convert_command(text, &vars),
            "---\ndescription: d\n---\nDo it\n\nARGUMENTS: $ARGUMENTS\n"
        );
        assert_eq!(convert_command("Use $1", &vars), "Use $1");
        assert_eq!(convert_command("Use ${@:2}", &vars), "Use ${@:2}");
    }
}
