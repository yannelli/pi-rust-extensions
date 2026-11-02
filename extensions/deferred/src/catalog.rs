//! What one run defers: the searchable items, the tool loadout, and the prompt patch.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::config::{Config, Kind, Mode};

/// The search tool of this extension; rules never apply to it.
pub const TOOL: &str = "deferred_search";
/// The prompt section this extension writes.
pub const SECTION: &str = "deferred";

#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    Tool,
    Skill { file: String, base_dir: String },
    Text(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub kind: Kind,
    /// Tool, skill, or section name, or the context file path.
    pub name: String,
    pub description: String,
    pub mode: Mode,
    /// Estimated prompt tokens the item would cost when loaded.
    pub tokens: usize,
    pub body: Body,
}

pub struct Inputs<'a> {
    pub config: &'a Config,
    /// `tools.getAll`.
    pub tools: &'a [Value],
    pub active: &'a [String],
    /// `systemPromptOptions` of `before_agent_start`.
    pub options: &'a Value,
    pub context_window: Option<u64>,
    /// Tools the model loaded this session.
    pub loaded: &'a BTreeSet<String>,
    /// Tools this extension took out of the loadout this session.
    pub removed: &'a BTreeSet<String>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    /// Searchable items that are not loaded.
    pub items: Vec<Item>,
    /// Disabled items with their estimated tokens.
    pub off: Vec<(Kind, String, usize)>,
    /// The tool loadout for the run.
    pub active: Vec<String>,
    /// Managed tools that something else put back in the loadout; they count as loaded.
    pub adopted: Vec<String>,
    pub newly_removed: Vec<String>,
    /// Tools this extension removed that are `on` again.
    pub restored: Vec<String>,
    pub skills: Option<Vec<Value>>,
    pub context_files: Option<Vec<Value>>,
    pub drop_sections: Vec<String>,
    pub auto: bool,
}

pub fn tokens(chars: usize) -> usize {
    chars.div_ceil(4)
}

pub fn plan(input: &Inputs) -> Plan {
    let mut plan = Plan {
        active: input.active.to_vec(),
        ..Plan::default()
    };
    plan_tools(input, &mut plan);
    plan_skills(input, &mut plan);
    plan_context(input, &mut plan);
    plan_sections(input, &mut plan);
    let has_tool = input.tools.iter().any(|tool| str_of(tool, "name") == TOOL);
    let listed = plan.active.iter().any(|name| name == TOOL);
    match has_tool && !plan.items.is_empty() {
        true if !listed => plan.active.push(TOOL.to_owned()),
        true => {}
        false => plan.active.retain(|name| name != TOOL),
    }
    plan
}

fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// Extension and MCP tools, which `auto` may defer.
fn is_addon(tool: &Value) -> bool {
    let source = tool.get("sourceInfo");
    let path = source.map_or("", |s| str_of(s, "path"));
    let origin = source.map_or("", |s| str_of(s, "source"));
    path == "builtin:mcp" || (!origin.is_empty() && origin != "builtin")
}

fn tool_tokens(tool: &Value) -> usize {
    let schema = tool.get("parameters").map_or(0, |p| p.to_string().len());
    tokens(str_of(tool, "name").len() + str_of(tool, "description").len() + schema)
}

fn tool_modes<'a>(input: &Inputs<'a>, plan: &mut Plan) -> Vec<(&'a Value, Mode)> {
    let candidates = input.tools.iter().filter(|tool| {
        let name = str_of(tool, "name");
        !name.is_empty() && name != TOOL && str_of(tool, "exposure") != "hidden"
    });
    let mut modes: Vec<(&Value, Option<Mode>)> = candidates
        .map(|tool| (tool, input.config.mode(Kind::Tool, str_of(tool, "name"))))
        .collect();
    let auto_eligible = |tool: &Value, mode: Option<Mode>| {
        let exposure = str_of(tool, "exposure");
        mode.is_none() && is_addon(tool) && (exposure.is_empty() || exposure == "direct")
    };
    if let (Some(percent), Some(window)) = (input.config.auto_percent, input.context_window) {
        let cost: usize = modes
            .iter()
            .filter(|(tool, mode)| auto_eligible(tool, *mode))
            .map(|(tool, _)| tool_tokens(tool))
            .sum();
        if cost as f64 * 100.0 > percent * window as f64 {
            plan.auto = true;
            for (tool, mode) in &mut modes {
                if auto_eligible(tool, *mode) {
                    *mode = Some(Mode::Names);
                }
            }
        }
    }
    modes
        .into_iter()
        .map(|(tool, mode)| (tool, mode.unwrap_or_default()))
        .collect()
}

fn plan_tools(input: &Inputs, plan: &mut Plan) {
    for (tool, mode) in tool_modes(input, plan) {
        let name = str_of(tool, "name");
        let is_active = plan.active.iter().any(|n| n == name);
        match mode {
            Mode::On => {
                if !is_active && input.removed.contains(name) {
                    plan.active.push(name.to_owned());
                    plan.restored.push(name.to_owned());
                }
                continue;
            }
            Mode::Off => {
                if is_active {
                    plan.active.retain(|n| n != name);
                    plan.newly_removed.push(name.to_owned());
                }
                plan.off
                    .push((Kind::Tool, name.to_owned(), tool_tokens(tool)));
                continue;
            }
            Mode::Names | Mode::Deferred => {}
        }
        if input.loaded.contains(name) {
            if !is_active {
                plan.active.push(name.to_owned());
            }
            continue;
        }
        if is_active && input.removed.contains(name) {
            plan.adopted.push(name.to_owned());
            continue;
        }
        if is_active {
            plan.active.retain(|n| n != name);
            plan.newly_removed.push(name.to_owned());
        }
        plan.items.push(Item {
            kind: Kind::Tool,
            name: name.to_owned(),
            description: str_of(tool, "description").to_owned(),
            mode,
            tokens: tool_tokens(tool),
            body: Body::Tool,
        });
    }
}

fn plan_skills(input: &Inputs, plan: &mut Plan) {
    let Some(skills) = input.options.get("skills").and_then(Value::as_array) else {
        return;
    };
    let mut keep = Vec::new();
    for skill in skills {
        let (name, description) = (str_of(skill, "name"), str_of(skill, "description"));
        let hidden = skill.get("disableModelInvocation").and_then(Value::as_bool) == Some(true);
        let mode = match hidden {
            true => Mode::On,
            false => input.config.mode(Kind::Skill, name).unwrap_or_default(),
        };
        let file = str_of(skill, "filePath");
        let cost = tokens(name.len() + description.len() + file.len() + 60);
        match mode {
            Mode::On => keep.push(skill.clone()),
            Mode::Off => plan.off.push((Kind::Skill, name.to_owned(), cost)),
            Mode::Names | Mode::Deferred => plan.items.push(Item {
                kind: Kind::Skill,
                name: name.to_owned(),
                description: description.to_owned(),
                mode,
                tokens: cost,
                body: Body::Skill {
                    file: file.to_owned(),
                    base_dir: str_of(skill, "baseDir").to_owned(),
                },
            }),
        }
    }
    if keep.len() != skills.len() {
        plan.skills = Some(keep);
    }
}

/// The first non-empty line without heading marks, at most 120 characters.
pub fn summary_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty());
    let line = line.unwrap_or_default().trim_start_matches('#').trim();
    match line.char_indices().nth(120) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_owned(),
    }
}

fn plan_context(input: &Inputs, plan: &mut Plan) {
    let Some(files) = input.options.get("contextFiles").and_then(Value::as_array) else {
        return;
    };
    let mut keep = Vec::new();
    for file in files {
        let (path, content) = (str_of(file, "path"), str_of(file, "content"));
        let mode = input.config.mode(Kind::Context, path).unwrap_or_default();
        let cost = tokens(path.len() + content.len() + 50);
        match mode {
            Mode::On => keep.push(file.clone()),
            Mode::Off => plan.off.push((Kind::Context, path.to_owned(), cost)),
            Mode::Names | Mode::Deferred => {
                plan.items
                    .push(text_item(Kind::Context, path, content, mode, cost))
            }
        }
    }
    if keep.len() != files.len() {
        plan.context_files = Some(keep);
    }
}

fn plan_sections(input: &Inputs, plan: &mut Plan) {
    let Some(sections) = input.options.get("sections").and_then(Value::as_object) else {
        return;
    };
    for (name, text) in sections {
        let Some(text) = text.as_str().filter(|t| !t.is_empty() && name != SECTION) else {
            continue;
        };
        let mode = input.config.mode(Kind::Section, name).unwrap_or_default();
        let cost = tokens(name.len() * 2 + text.len() + 6);
        match mode {
            Mode::On => continue,
            Mode::Off => plan.off.push((Kind::Section, name.clone(), cost)),
            Mode::Names | Mode::Deferred => {
                plan.items
                    .push(text_item(Kind::Section, name, text, mode, cost))
            }
        }
        plan.drop_sections.push(name.clone());
    }
}

fn text_item(kind: Kind, name: &str, text: &str, mode: Mode, tokens: usize) -> Item {
    Item {
        kind,
        name: name.to_owned(),
        description: summary_line(text),
        mode,
        tokens,
        body: Body::Text(text.to_owned()),
    }
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
