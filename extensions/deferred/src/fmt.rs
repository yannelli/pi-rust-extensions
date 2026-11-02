//! Text for the prompt index, `deferred_search` results, and `/deferred`.

use std::collections::BTreeMap;

use crate::catalog::{Body, Item, Plan, TOOL};
use crate::config::{Config, Kind, Mode};

/// The `deferred` prompt section; `None` when nothing is searchable.
pub fn index(items: &[Item]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let mut lines = vec![format!(
        "Some tools, skills, and instructions are deferred to save context. Call `{TOOL}` to load them: \
         `select:<name>[,<name>]` loads exact names, and keywords search names and descriptions \
         (`+word` requires a word). A deferred tool cannot be called until it is loaded."
    )];
    for kind in Kind::ALL {
        let names: Vec<&str> = items
            .iter()
            .filter(|i| i.kind == kind && i.mode == Mode::Names)
            .map(|i| i.name.as_str())
            .collect();
        if !names.is_empty() {
            let list = match kind {
                Kind::Tool => group_tools(&names),
                _ => names.join(", "),
            };
            lines.push(format!("Deferred {}s: {list}", kind.label()));
        }
    }
    let hidden = items.iter().filter(|i| i.mode == Mode::Deferred).count();
    if hidden > 0 {
        lines.push(format!(
            "{hidden} more deferred items are searchable but not listed."
        ));
    }
    Some(lines.join("\n"))
}

/// `mcp__<server>__<tool>` names grouped as `mcp__<server>__{a, b}`.
fn group_tools(names: &[&str]) -> String {
    let mut groups: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut plain = Vec::new();
    for name in names {
        let server = name
            .strip_prefix("mcp__")
            .and_then(|rest| rest.split_once("__"));
        match server {
            Some((server, tool)) if !tool.is_empty() => {
                groups.entry(server).or_default().push(tool)
            }
            _ => plain.push((*name).to_owned()),
        }
    }
    for (server, tools) in groups {
        plain.push(match tools.as_slice() {
            [one] => format!("mcp__{server}__{one}"),
            _ => format!("mcp__{server}__{{{}}}", tools.join(", ")),
        });
    }
    plain.join(", ")
}

/// Strips a leading `---` frontmatter block.
pub fn skill_body(content: &str) -> &str {
    let Some(rest) = content.strip_prefix("---") else {
        return content;
    };
    let mut lines = rest.split_inclusive('\n');
    let first = lines.next().unwrap_or_default();
    if !first.trim().is_empty() {
        return content;
    }
    let mut offset = content.len() - rest.len() + first.len();
    for line in lines {
        offset += line.len();
        if line.trim_end() == "---" {
            return &content[offset..];
        }
    }
    content
}

/// One loaded item as the model sees it; tools only get a line.
pub fn loaded(item: &Item, skill_text: Option<&str>) -> String {
    match &item.body {
        Body::Tool => format!("- {}: {}", item.name, item.description),
        Body::Skill { file, base_dir } => {
            let body = skill_text.map_or("(the skill file could not be read)", |t| {
                skill_body(t).trim()
            });
            format!(
                "<skill name=\"{}\" location=\"{file}\">\nReferences are relative to {base_dir}.\n\n{body}\n</skill>",
                item.name
            )
        }
        Body::Text(text) => match item.kind {
            Kind::Context => format!(
                "<project_instructions path=\"{}\">\n{text}\n</project_instructions>",
                item.name
            ),
            _ => format!("<{0}>\n{text}\n</{0}>", item.name),
        },
    }
}

pub fn no_match(query: &str, items: &[Item]) -> String {
    let mut counts: BTreeMap<Kind, usize> = BTreeMap::new();
    items
        .iter()
        .for_each(|i| *counts.entry(i.kind).or_default() += 1);
    let counts: Vec<String> = counts
        .iter()
        .map(|(k, n)| format!("{n} {}s", k.label()))
        .collect();
    match counts.is_empty() {
        true => "Nothing is deferred.".to_owned(),
        false => format!(
            "No deferred item matches `{query}`. Deferred: {}. Try other keywords.",
            counts.join(", ")
        ),
    }
}

/// The `/deferred` report.
pub fn status(
    config: &Config,
    sources: &[String],
    plan: Option<&Plan>,
    loaded: &[String],
) -> String {
    let mut lines = vec![format!("deferred config: {}", sources.join(", "))];
    lines.push(match config.auto_percent {
        Some(p) => {
            format!("auto: extension and MCP tools load as names past {p}% of the context window")
        }
        None => "auto: off".to_owned(),
    });
    lines.push(format!("rules: {}", config.rules.len()));
    for rule in &config.rules {
        lines.push(format!(
            "  {} {} = {}",
            rule.kind.key(),
            rule.pattern,
            rule.mode.as_str()
        ));
    }
    config
        .warnings
        .iter()
        .for_each(|w| lines.push(format!("warning: {w}")));
    let Some(plan) = plan else {
        lines.push("No run yet: counts show after the next prompt.".to_owned());
        return lines.join("\n");
    };
    if plan.auto {
        lines.push("auto deferral is active this run".to_owned());
    }
    let mut saved = 0;
    for kind in Kind::ALL {
        let of = |mode: Mode| {
            plan.items
                .iter()
                .filter(|i| i.kind == kind && i.mode == mode)
                .collect::<Vec<_>>()
        };
        let (names, hidden) = (of(Mode::Names), of(Mode::Deferred));
        let off: Vec<&(Kind, String, usize)> = plan.off.iter().filter(|o| o.0 == kind).collect();
        let tokens: usize = names.iter().chain(&hidden).map(|i| i.tokens).sum::<usize>()
            + off.iter().map(|o| o.2).sum::<usize>();
        saved += tokens;
        if names.len() + hidden.len() + off.len() == 0 {
            continue;
        }
        lines.push(format!(
            "{}s: {} names, {} deferred, {} off (~{tokens} tokens)",
            kind.label(),
            names.len(),
            hidden.len(),
            off.len()
        ));
        let mut listed: Vec<String> = names
            .iter()
            .chain(&hidden)
            .map(|i| i.name.clone())
            .collect();
        listed.extend(off.iter().map(|o| format!("{} (off)", o.1)));
        lines.push(format!("  {}", clip(&listed, 12)));
    }
    lines.push(format!("estimated tokens kept out of the prompt: ~{saved}"));
    if !loaded.is_empty() {
        lines.push(format!("loaded tools this session: {}", clip(loaded, 20)));
    }
    lines.join("\n")
}

fn clip(names: &[String], max: usize) -> String {
    let shown = names
        .iter()
        .take(max)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    match names.len().saturating_sub(max) {
        0 => shown,
        more => format!("{shown}, +{more} more"),
    }
}

#[cfg(test)]
#[path = "fmt_tests.rs"]
mod tests;
