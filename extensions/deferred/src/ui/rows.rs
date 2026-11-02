//! Everything the `/deferred` panel lists: each item with its mode and why, and each rule.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::catalog::{Inputs, SECTION, tool_modes};
use crate::config::{Config, Kind, Mode, Rule, SETTING_SOURCE};
use crate::measure::{
    context_tokens, section_tokens, skill_tokens, str_of, summary_line, tool_tokens,
};

/// A settings file the panel can write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Global,
    Project,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Self::Global => "global settings",
            Self::Project => "project settings",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Why {
    Default,
    /// `auto` made the tool `names`.
    Auto,
    /// `scope` is `None` for a rule in a `deferred.json` file, which the panel cannot edit.
    Rule {
        pattern: String,
        source: String,
        scope: Option<Scope>,
    },
    /// pi decides, whatever the rules say.
    Pinned(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub kind: Kind,
    pub name: String,
    pub description: String,
    pub mode: Mode,
    pub why: Why,
    pub loaded: bool,
    pub tokens: usize,
    pub details: Vec<String>,
}

impl Row {
    /// The settings scope of a rule that names this item exactly.
    pub fn exact_scope(&self) -> Option<Scope> {
        match &self.why {
            Why::Rule { pattern, scope, .. } if *pattern == self.name => *scope,
            _ => None,
        }
    }

    pub fn pinned(&self) -> bool {
        matches!(self.why, Why::Pinned(_))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuleRow {
    pub kind: Kind,
    pub pattern: String,
    pub mode: Mode,
    pub source: String,
    pub scope: Option<Scope>,
    pub matches: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inventory {
    pub rows: Vec<Row>,
    pub rules: Vec<RuleRow>,
    pub auto: bool,
}

impl Inventory {
    pub fn count(&self, kind: Kind) -> usize {
        self.rows.iter().filter(|r| r.kind == kind).count()
    }

    /// Estimated tokens of items that are not `on`.
    pub fn saved(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| r.mode != Mode::On && !r.loaded)
            .map(|r| r.tokens)
            .sum()
    }
}

pub struct Sources<'a> {
    pub config: &'a Config,
    pub tools: &'a [Value],
    pub options: &'a Value,
    pub context_window: Option<u64>,
    pub loaded: &'a BTreeSet<String>,
    /// The `deferred` blocks of the global and project `settings.json`.
    pub global: Option<&'a Value>,
    pub project: Option<&'a Value>,
}

fn scope_of(rule: &Rule, sources: &Sources) -> Option<Scope> {
    if rule.source != SETTING_SOURCE {
        return None;
    }
    let has = |block: Option<&Value>| {
        block
            .and_then(|b| b.get(rule.kind.key()))
            .and_then(|m| m.get(&rule.pattern))
            .is_some()
    };
    Some(if has(sources.project) {
        Scope::Project
    } else {
        Scope::Global
    })
}

fn why(sources: &Sources, kind: Kind, name: &str) -> Why {
    match sources.config.rule(kind, name) {
        Some(rule) => Why::Rule {
            pattern: rule.pattern.clone(),
            source: rule.source.clone(),
            scope: scope_of(rule, sources),
        },
        None => Why::Default,
    }
}

pub fn build(sources: &Sources) -> Inventory {
    let mut inventory = Inventory::default();
    let none = BTreeSet::new();
    let input = Inputs {
        config: sources.config,
        tools: sources.tools,
        active: &[],
        options: sources.options,
        context_window: sources.context_window,
        loaded: &none,
        removed: &none,
    };
    let (modes, auto) = tool_modes(&input);
    inventory.auto = auto;
    for (tool, mode) in modes {
        let name = str_of(tool, "name");
        let exposure = str_of(tool, "exposure");
        let mut why = why(sources, Kind::Tool, name);
        if why == Why::Default && mode != Mode::On {
            why = Why::Auto;
        } else if why == Why::Default && matches!(exposure, "codemode" | "deferred") {
            why = Why::Pinned("pi keeps it undeclared (MCP exposure)");
        }
        let path = tool.get("sourceInfo").map_or("", |s| str_of(s, "path"));
        let mut details = vec![format!("Source: {path}")];
        if !exposure.is_empty() {
            details.push(format!("Exposure: {exposure}"));
        }
        inventory.rows.push(Row {
            kind: Kind::Tool,
            name: name.to_owned(),
            description: str_of(tool, "description").to_owned(),
            mode,
            why,
            loaded: sources.loaded.contains(name),
            tokens: tool_tokens(tool),
            details,
        });
    }
    for skill in sources
        .options
        .get("skills")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = str_of(skill, "name");
        let hidden = skill.get("disableModelInvocation").and_then(Value::as_bool) == Some(true);
        let (mode, why) = match hidden {
            true => (
                Mode::On,
                Why::Pinned("disable-model-invocation: pi never lists it"),
            ),
            false => (
                sources.config.mode(Kind::Skill, name).unwrap_or_default(),
                why(sources, Kind::Skill, name),
            ),
        };
        let details = vec![format!("File: {}", str_of(skill, "filePath"))];
        let description = str_of(skill, "description").to_owned();
        let tokens = skill_tokens(skill);
        inventory.rows.push(Row {
            kind: Kind::Skill,
            name: name.to_owned(),
            description,
            mode,
            why,
            loaded: false,
            tokens,
            details,
        });
    }
    for file in sources
        .options
        .get("contextFiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (path, content) = (str_of(file, "path"), str_of(file, "content"));
        let details = vec![format!("Size: {} bytes", content.len())];
        let mode = sources.config.mode(Kind::Context, path).unwrap_or_default();
        inventory.rows.push(Row {
            kind: Kind::Context,
            name: path.to_owned(),
            description: summary_line(content),
            mode,
            why: why(sources, Kind::Context, path),
            loaded: false,
            tokens: context_tokens(path, content),
            details,
        });
    }
    let sections = sources.options.get("sections").and_then(Value::as_object);
    for (name, text) in sections.into_iter().flatten() {
        let Some(text) = text.as_str().filter(|t| !t.is_empty() && name != SECTION) else {
            continue;
        };
        inventory.rows.push(Row {
            kind: Kind::Section,
            name: name.clone(),
            description: summary_line(text),
            mode: sources.config.mode(Kind::Section, name).unwrap_or_default(),
            why: why(sources, Kind::Section, name),
            loaded: false,
            tokens: section_tokens(name, text),
            details: vec![format!("Size: {} bytes", text.len())],
        });
    }
    for rule in &sources.config.rules {
        let matches = inventory
            .rows
            .iter()
            .filter(|row| {
                row.kind == rule.kind
                    && sources
                        .config
                        .rule(row.kind, &row.name)
                        .is_some_and(|r| std::ptr::eq(r, rule))
            })
            .count();
        inventory.rules.push(RuleRow {
            kind: rule.kind,
            pattern: rule.pattern.clone(),
            mode: rule.mode,
            source: rule.source.clone(),
            scope: scope_of(rule, sources),
            matches,
        });
    }
    inventory
}
