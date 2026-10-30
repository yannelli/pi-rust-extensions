use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::frontmatter::{Doc, substitute};
use crate::source::ensure_inside;
use crate::stage::{Target, markdown_files};

/// Claude Code tool name to pi tool name; other names stay.
pub fn map_tool(name: &str) -> String {
    let base = name.split('(').next().unwrap_or(name).trim();
    let mapped = match base {
        "Bash" => "bash",
        "Read" => "read",
        "Edit" => "edit",
        "Write" => "write",
        "Grep" => "grep",
        "Glob" => "find",
        "LS" => "ls",
        "PowerShell" => "powershell",
        _ => return name.trim().to_owned(),
    };
    mapped.to_owned()
}

fn map_tools(list: Vec<String>) -> String {
    let mut out: Vec<String> = Vec::new();
    for tool in list.iter().map(|t| map_tool(t)) {
        if !out.contains(&tool) {
            out.push(tool);
        }
    }
    out.join(", ")
}

/// Rewrites an agent file: `name` to the staged name, tool lists to pi names.
pub fn convert(text: &str, staged: &str, vars: &[(&str, String)]) -> String {
    let mut doc = Doc::parse(text);
    doc.set("name", staged);
    for key in ["tools", "disallowedTools"] {
        if let Some(list) = doc.get_list(key) {
            doc.set(key, &map_tools(list));
        }
    }
    substitute(&doc.render(), vars)
}

pub fn stage_agents(target: &Target, paths: &[PathBuf], files: &mut Vec<PathBuf>) -> Result<usize> {
    let dir = target.dirs.agents();
    let vars = target.vars(None);
    let mut count = 0;
    for path in paths {
        for (parts, file) in markdown_files(path) {
            let Some(name) = parts.last() else { continue };
            let staged = target.prefixed(name);
            let dest = dir.join(format!("{staged}.md"));
            ensure_inside(&dir, &dest)?;
            if files.contains(&dest) {
                continue;
            }
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&dest, convert(&text, &staged, &vars))?;
            files.push(dest);
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_tool_names() {
        let text = "---\nname: rev\ntools: Bash, Read, Glob, Bash(git:*), WebFetch\n---\nUse ${CLAUDE_PLUGIN_ROOT}\n";
        let vars = [("CLAUDE_PLUGIN_ROOT", "/p".to_owned())];
        let out = convert(text, "fmt-rev", &vars);
        assert_eq!(
            out,
            "---\nname: fmt-rev\ntools: bash, read, find, WebFetch\n---\nUse /p\n"
        );
    }
}
