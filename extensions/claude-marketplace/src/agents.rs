use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::fields::FrontDrops;
use crate::frontmatter::{Doc, substitute};
use crate::settings::Rule;
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

/// Who the agent is, for the `agentModels` rules.
pub struct AgentId<'a> {
    pub plugin: &'a str,
    /// The Claude name and the staged name.
    pub names: [&'a str; 2],
}

/// Rewrites an agent file: `name` to the staged name, tool lists to pi names, and `model` and
/// `effort` by the first matching rule, else the built-in mapping. Returns the `info` line.
pub fn convert(text: &str, id: &AgentId, rules: &[Rule], vars: &Vars) -> (String, String) {
    let mut doc = Doc::parse(text);
    let staged = id.names[1];
    doc.set("name", staged);
    for key in ["tools", "disallowedTools"] {
        if let Some(list) = doc.get_list(key) {
            doc.set(key, &map_tools(list));
        }
    }
    let claude_model = doc.get("model").filter(|m| !m.trim().is_empty());
    let effort = doc.get("effort").unwrap_or_default();
    let shown_model = claude_model.clone().unwrap_or_else(|| "inherit".into());
    let rule = rules
        .iter()
        .find(|r| r.matches(id.plugin, &id.names, &shown_model, &effort));
    let builtin_thinking = doc
        .get("thinking")
        .or_else(|| effort_thinking(&effort).map(str::to_owned));
    let model = rule
        .and_then(|r| r.to_model.clone())
        .or_else(|| claude_model.as_deref().map(map_model));
    let thinking = rule.and_then(|r| r.thinking.clone()).or(builtin_thinking);
    if let Some(model) = &model {
        doc.set("model", model);
    }
    if let Some(thinking) = &thinking {
        doc.set("thinking", thinking);
    }
    let effort_text = if effort.is_empty() { "none" } else { &effort };
    let line = format!(
        "{staged}: model {shown_model}, effort {effort_text} -> model {}, thinking {} ({})",
        model.as_deref().unwrap_or("inherit"),
        thinking.as_deref().unwrap_or("inherit"),
        rule.map_or("built-in", |r| r.source.as_str()),
    );
    (substitute(&doc.render(), vars), line)
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

/// Stages the agents; `models` gets one `info` line per agent.
pub fn stage_agents(
    target: &Target,
    paths: &[PathBuf],
    files: &mut Vec<PathBuf>,
    drops: &mut FrontDrops,
    models: &mut Vec<String>,
) -> Result<usize> {
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
            drops.add("agent", &staged, &Doc::parse(&text).keys());
            let id = AgentId {
                plugin: target.plugin,
                names: [&name, &staged],
            };
            let (converted, line) = convert(&text, &id, &target.rules, &vars);
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&dest, converted)?;
            models.push(line);
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
        let id = AgentId {
            plugin: "fmt",
            names: ["rev", "fmt-rev"],
        };
        let (out, line) = convert(text, &id, &[], &vars);
        assert_eq!(
            out,
            "---\nname: fmt-rev\ntools: bash, read, find, url_context, mcp__x__y\nmodel: inherit\neffort: max\nthinking: xhigh\n---\nUse /p\n"
        );
        assert_eq!(
            line,
            "fmt-rev: model fable, effort max -> model inherit, thinking xhigh (built-in)"
        );
        let rule = Rule {
            agent: Some("rev".into()),
            model: Some("fa*".into()),
            to_model: Some("openai/gpt-5".into()),
            source: "rule 1".into(),
            ..Rule::default()
        };
        let (out, line) = convert(text, &id, &[rule], &vars);
        assert!(out.contains("model: openai/gpt-5\n"), "{out}");
        assert!(
            line.ends_with("-> model openai/gpt-5, thinking xhigh (rule 1)"),
            "{line}"
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
