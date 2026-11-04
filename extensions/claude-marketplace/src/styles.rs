//! Claude plugin output styles: staged as Markdown files, and the selected style goes into the
//! system prompt as a section.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::json;

use crate::config::Config;
use crate::env::{Env, Scope};
use crate::fields::FrontDrops;
use crate::frontmatter::{Doc, substitute};
use crate::source::ensure_inside;
use crate::stage::{COMMAND_SEPARATOR, Target, markdown_files};

/// The system prompt section key.
pub const SECTION: &str = "claude-output-style";
/// The config key that holds the selected style.
const CONFIG_KEY: &str = "outputStyle";

/// One staged style.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub name: String,
    pub description: String,
    pub forced: bool,
    pub body: String,
    pub scope: Scope,
}

pub fn stage_styles(
    target: &Target,
    paths: &[PathBuf],
    files: &mut Vec<PathBuf>,
    drops: &mut FrontDrops,
) -> Result<usize> {
    let dir = target.dirs.output_styles();
    let vars = target.vars(None);
    let mut count = 0;
    for path in paths {
        for (parts, file) in markdown_files(path) {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            let mut doc = Doc::parse(&text);
            let Some(stem) = parts.last() else { continue };
            let own = doc.get("name").filter(|n| !n.trim().is_empty());
            let name = own
                .unwrap_or_else(|| stem.clone())
                .replace(['/', '\\', ':'], "-");
            let staged = format!("{}{COMMAND_SEPARATOR}{name}", target.plugin);
            let dest = dir.join(format!("{staged}.md"));
            ensure_inside(&dir, &dest)?;
            if files.contains(&dest) {
                continue;
            }
            drops.add("style", &staged, &doc.keys());
            doc.set("name", &format!("{}:{name}", target.plugin));
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&dest, substitute(&doc.render(), &vars))?;
            files.push(dest);
            count += 1;
        }
    }
    Ok(count)
}

fn read_style(path: &Path, scope: Scope) -> Option<Style> {
    let doc = Doc::parse(&std::fs::read_to_string(path).ok()?);
    let flag = |key: &str| doc.get(key).is_some_and(|v| v.eq_ignore_ascii_case("true"));
    Some(Style {
        name: doc.get("name")?,
        description: doc.get("description").unwrap_or_default(),
        forced: flag("force-for-plugin"),
        body: doc.body.trim().to_owned(),
        scope,
    })
}

/// The staged styles of the visible scopes, project first, sorted by name in each scope.
pub fn all(env: &Env) -> Vec<Style> {
    let mut out: Vec<Style> = Vec::new();
    for scope in env.visible_scopes() {
        let dir = env.dirs(scope).output_styles();
        for (_, file) in markdown_files(&dir) {
            if let Some(style) = read_style(&file, scope) {
                if !out.iter().any(|s| s.name == style.name) {
                    out.push(style);
                }
            }
        }
    }
    out
}

/// The selected style name: the project config, then the user config.
fn selected(env: &Env) -> Option<String> {
    env.visible_scopes().into_iter().find_map(|scope| {
        let config = Config::load(&env.dirs(scope)).ok()?;
        config.get(CONFIG_KEY)?.as_str().map(str::to_owned)
    })
}

/// The active style: the first one with `force-for-plugin`, else the selected one.
pub fn active(env: &Env) -> Option<Style> {
    let styles = all(env);
    if let Some(forced) = styles.iter().find(|s| s.forced) {
        return Some(forced.clone());
    }
    let name = selected(env)?;
    styles.into_iter().find(|s| s.name == name)
}

/// The system prompt section of the active style.
pub fn section(env: &Env) -> Option<String> {
    let style = active(env)?;
    Some(format!("# Output style: {}\n\n{}", style.name, style.body))
}

/// `/claude-plugins style [<name> | off]`.
pub fn command(env: &Env, name: Option<&str>, scope: Option<Scope>) -> Result<String> {
    let styles = all(env);
    let Some(name) = name else {
        if styles.is_empty() {
            return Ok("No output styles are installed.".into());
        }
        let active = active(env).map(|s| s.name);
        let mut lines = vec!["Output styles:".to_owned()];
        for style in &styles {
            let mark = if active.as_ref() == Some(&style.name) {
                "*"
            } else {
                " "
            };
            let forced = if style.forced {
                " (forced by plugin)"
            } else {
                ""
            };
            lines.push(format!(
                "{mark} {}{forced} - {}",
                style.name, style.description
            ));
        }
        lines.push("Select one with /claude-plugins style <name>, or style off.".into());
        return Ok(lines.join("\n"));
    };
    let scope = scope.unwrap_or(Scope::User);
    env.require(scope)?;
    let mut config = Config::load(&env.dirs(scope))?;
    if name == "off" {
        config.set(CONFIG_KEY, None);
        config.save()?;
        return Ok(format!("Output style off [{scope}]."));
    }
    if !styles.iter().any(|s| s.name == name) {
        bail!("output style \"{name}\" is not installed. Try /claude-plugins style");
    }
    config.set(CONFIG_KEY, Some(json!(name)));
    config.save()?;
    let mut text = format!("Output style {name} [{scope}]: it applies from the next prompt.");
    if let Some(forced) = styles.iter().find(|s| s.forced && s.name != name) {
        text.push_str(&format!(
            " {} is forced by its plugin and wins.",
            forced.name
        ));
    }
    Ok(text)
}

/// Style names, for completions.
pub fn style_names(env: &Env) -> Vec<String> {
    all(env).into_iter().map(|s| s.name).collect()
}
