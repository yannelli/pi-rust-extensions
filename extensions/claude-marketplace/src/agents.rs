use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::frontmatter::{Doc, substitute};
use crate::source::ensure_inside;
use crate::stage::{Target, Vars, markdown_files};

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
        "WebFetch" => "url_context",
        "WebSearch" => "web_search",
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

/// A Claude `model` for the subagents extension: the Claude aliases inherit the session model,
/// a bare Claude model id gets the `anthropic/` provider, and other values stay.
pub fn map_model(model: &str) -> String {
    let model = model.trim();
    let base = model.split('[').next().unwrap_or(model);
    let aliases = [
        "inherit", "default", "sonnet", "opus", "haiku", "fable", "opusplan",
    ];
    if model.is_empty() || aliases.iter().any(|a| base.eq_ignore_ascii_case(a)) {
        "inherit".into()
    } else if model.starts_with("claude-") {
        format!("anthropic/{model}")
    } else {
        model.to_owned()
    }
}

/// Claude `effort` as a pi thinking level.
fn effort_thinking(effort: &str) -> Option<&'static str> {
    match effort.trim().to_ascii_lowercase().as_str() {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" | "max" => Some("xhigh"),
        _ => None,
    }
}

/// Rewrites an agent file: `name` to the staged name, tool lists to pi names, `model` to a
/// subagents model, and `effort` to `thinking`.
pub fn convert(text: &str, staged: &str, vars: &Vars) -> String {
    let mut doc = Doc::parse(text);
    doc.set("name", staged);
    for key in ["tools", "disallowedTools"] {
        if let Some(list) = doc.get_list(key) {
            doc.set(key, &map_tools(list));
        }
    }
    if let Some(model) = doc.get("model") {
        doc.set("model", &map_model(&model));
    }
    let effort = doc.get("effort").as_deref().and_then(effort_thinking);
    if let (Some(level), None) = (effort, doc.get("thinking")) {
        doc.set("thinking", level);
    }
    substitute(&doc.render(), vars)
}

/// The agent name: frontmatter `name` (else the file name), after its subfolders.
fn agent_name(parts: &[String], text: &str) -> Option<String> {
    let (last, dirs) = parts.split_last()?;
    let own = Doc::parse(text)
        .get("name")
        .filter(|n| !n.trim().is_empty());
    let mut all: Vec<String> = dirs.to_vec();
    all.push(own.unwrap_or_else(|| last.clone()));
    let name: String = all
        .join("-")
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            c @ ('a'..='z' | '0'..='9' | '_') => c,
            _ => '-',
        })
        .collect();
    Some(name.trim_matches('-').to_owned()).filter(|n| !n.is_empty())
}

pub fn stage_agents(target: &Target, paths: &[PathBuf], files: &mut Vec<PathBuf>) -> Result<usize> {
    let dir = target.dirs.agents();
    let vars = target.vars(None);
    let mut count = 0;
    for path in paths {
        for (parts, file) in markdown_files(path) {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            let Some(name) = agent_name(&parts, &text) else {
                continue;
            };
            let staged = target.prefixed(&name);
            let dest = dir.join(format!("{staged}.md"));
            ensure_inside(&dir, &dest)?;
            if files.contains(&dest) {
                continue;
            }
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
        let text = "---\nname: rev\ntools: Bash, Read, Glob, Bash(git:*), WebFetch, mcp__x__y\nmodel: fable\neffort: max\n---\nUse ${CLAUDE_PLUGIN_ROOT}\n";
        let vars = vec![("CLAUDE_PLUGIN_ROOT".to_owned(), "/p".to_owned())];
        let out = convert(text, "fmt-rev", &vars);
        assert_eq!(
            out,
            "---\nname: fmt-rev\ntools: bash, read, find, url_context, mcp__x__y\nmodel: inherit\neffort: max\nthinking: xhigh\n---\nUse /p\n"
        );
    }

    #[test]
    fn maps_models_and_names() {
        assert_eq!(map_model("opus[1m]"), "inherit");
        assert_eq!(
            map_model("claude-sonnet-4-6"),
            "anthropic/claude-sonnet-4-6"
        );
        assert_eq!(map_model("openai/gpt-5"), "openai/gpt-5");
        let parts = ["review".to_owned(), "audit".to_owned()];
        assert_eq!(agent_name(&parts, "body").as_deref(), Some("review-audit"));
        let named = "---\nname: Deep Audit\n---\n";
        assert_eq!(
            agent_name(&parts, named).as_deref(),
            Some("review-deep-audit")
        );
    }
}
