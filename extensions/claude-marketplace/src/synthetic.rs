//! A source without `.claude-plugin/marketplace.json` that is one plugin, one skill, or a
//! folder of skills becomes a marketplace with one plugin.

use std::path::Path;

use serde_json::{Value, json};

use crate::frontmatter::Doc;
use crate::plugin::read_manifest;
use crate::source::check_name;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// A plugin.json at the root.
    Plugin,
    /// A root `SKILL.md`.
    Skill,
    /// `skills/*/SKILL.md`.
    SkillsDir,
    /// `*/SKILL.md`.
    SkillDirs,
}

impl Layout {
    pub fn label(self) -> &'static str {
        match self {
            Layout::Plugin => "a plugin folder",
            Layout::Skill => "a single skill",
            Layout::SkillsDir | Layout::SkillDirs => "a folder of skills",
        }
    }
}

fn has_skill_dirs(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            let name = entry.file_name();
            !name.to_string_lossy().starts_with('.') && entry.path().join("SKILL.md").is_file()
        })
}

pub fn detect(root: &Path) -> Option<Layout> {
    if read_manifest(root).ok().flatten().is_some() {
        Some(Layout::Plugin)
    } else if root.join("SKILL.md").is_file() {
        Some(Layout::Skill)
    } else if has_skill_dirs(&root.join("skills")) {
        Some(Layout::SkillsDir)
    } else if has_skill_dirs(root) {
        Some(Layout::SkillDirs)
    } else {
        None
    }
}

/// A marketplace or plugin name from a dir, repo, or skill name.
pub fn name_from(raw: &str) -> String {
    let raw = raw.trim().trim_end_matches('/');
    let last = raw.rsplit(['/', ':', '\\']).next().unwrap_or(raw);
    let last = last.strip_suffix(".git").unwrap_or(last);
    let name: String = last
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            c @ ('a'..='z' | '0'..='9' | '-' | '_') => c,
            _ => '-',
        })
        .collect();
    let name = name.trim_matches('-').to_owned();
    if name.is_empty() {
        "skills".into()
    } else {
        name
    }
}

fn front(path: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    Doc::parse(&text).get(key).filter(|v| !v.trim().is_empty())
}

/// The marketplace.json of a synthetic marketplace named `name`, or `None` when `root` has
/// no plugin or skill layout.
pub fn catalog(root: &Path, name: &str) -> Option<(Value, Layout)> {
    let layout = detect(root)?;
    let manifest = read_manifest(root).ok().flatten();
    let text = |key: &str| manifest.as_ref()?.get(key)?.as_str().map(str::to_owned);
    let (plugin, description) = match layout {
        Layout::Plugin => (text("name"), text("description")),
        Layout::Skill => {
            let skill = root.join("SKILL.md");
            (
                front(&skill, "name").map(|n| name_from(&n)),
                front(&skill, "description"),
            )
        }
        _ => (None, None),
    };
    let plugin = plugin
        .filter(|p| check_name("plugin", p).is_ok())
        .unwrap_or_else(|| name.to_owned());
    let mut entry = json!({ "name": plugin, "source": "./" });
    if let Some(description) = description {
        entry["description"] = json!(description);
    }
    if layout == Layout::SkillDirs {
        entry["skills"] = json!(["./"]);
    }
    let value = json!({ "name": name, "owner": { "name": name }, "plugins": [entry] });
    Some((value, layout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_sources() {
        assert_eq!(
            name_from("https://github.com/acme/PDF_Skill.git"),
            "pdf_skill"
        );
        assert_eq!(name_from("git@github.com:acme/notes"), "notes");
        assert_eq!(name_from("/tmp/My Skills/"), "my-skills");
        assert_eq!(name_from("..."), "skills");
    }
}
