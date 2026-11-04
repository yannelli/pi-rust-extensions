//! The form of one agent: its fields, the unsaved values, validation, and the file changes.

use crate::agent::{AgentDef, Exposure, PromptMode};
use crate::frontmatter::{FmValue, Frontmatter};

pub const THINKING: [&str; 8] = [
    "inherit", "off", "minimal", "low", "medium", "high", "xhigh", "max",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Field {
    Name,
    Description,
    Model,
    Thinking,
    Tools,
    Disallowed,
    MaxTurns,
    PromptMode,
    Exposure,
    Prompt,
}

pub const FIELDS: [Field; 10] = [
    Field::Name,
    Field::Description,
    Field::Model,
    Field::Thinking,
    Field::Tools,
    Field::Disallowed,
    Field::MaxTurns,
    Field::PromptMode,
    Field::Exposure,
    Field::Prompt,
];

impl Field {
    pub fn label(self) -> &'static str {
        match self {
            Field::Name => "Name",
            Field::Description => "Description",
            Field::Model => "Model",
            Field::Thinking => "Thinking",
            Field::Tools => "Tools",
            Field::Disallowed => "Denied tools",
            Field::MaxTurns => "Max turns",
            Field::PromptMode => "Prompt mode",
            Field::Exposure => "Exposure",
            Field::Prompt => "Prompt",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            Field::Name => "Lowercase letters, digits, - and _. Press enter to rename (r).",
            Field::Description => {
                "When to use the agent. The parent model reads it to pick an agent, so say what it is for."
            }
            Field::Model => "provider/id, or inherit to use the session model and thinking level.",
            Field::Thinking => "←→ cycles the thinking level. inherit keeps the session level.",
            Field::Tools => "The tools of the child. all gives every tool except subagent.",
            Field::Disallowed => "Tools removed from the child, also when Tools is all.",
            Field::MaxTurns => "Stop the child after this many turns. Empty for no limit.",
            Field::PromptMode => "append adds the prompt to pi's system prompt; replace swaps it.",
            Field::Exposure => {
                "direct lists the agent in the system prompt; deferred leaves it to tool_search. inherit follows pi's contextExposure."
            }
            Field::Prompt => {
                "Enter opens the prompt in pi's editor; its external editor key opens $EDITOR."
            }
        }
    }

    /// Fields that ←→ cycle and enter steps.
    pub fn cycles(self) -> bool {
        matches!(self, Field::Thinking | Field::PromptMode | Field::Exposure)
    }

    /// Fields typed on the form row.
    pub fn inline(self) -> bool {
        matches!(self, Field::Description | Field::MaxTurns)
    }
}

/// The editable values of an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Values {
    pub description: String,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub tools: Option<Vec<String>>,
    pub disallowed: Vec<String>,
    pub max_turns: String,
    pub prompt_mode: PromptMode,
    pub exposure: Option<Exposure>,
    pub prompt: String,
}

impl Values {
    pub fn of(agent: &AgentDef) -> Self {
        Self {
            description: agent.description.clone(),
            model: agent.model.clone(),
            thinking: agent.thinking.clone(),
            tools: agent.tools.clone(),
            disallowed: agent.disallowed_tools.clone(),
            max_turns: agent.max_turns.map(|t| t.to_string()).unwrap_or_default(),
            prompt_mode: agent.prompt_mode,
            exposure: agent.exposure,
            prompt: agent.prompt.clone(),
        }
    }

    fn differs(&self, other: &Values, field: Field) -> bool {
        match field {
            Field::Name => false,
            Field::Description => self.description.trim() != other.description.trim(),
            Field::Model => self.model != other.model,
            Field::Thinking => self.thinking != other.thinking,
            Field::Tools => self.tools != other.tools,
            Field::Disallowed => self.disallowed != other.disallowed,
            Field::MaxTurns => self.max_turns.trim() != other.max_turns.trim(),
            Field::PromptMode => self.prompt_mode != other.prompt_mode,
            Field::Exposure => self.exposure != other.exposure,
            Field::Prompt => self.prompt.trim() != other.prompt.trim(),
        }
    }

    /// The frontmatter value of `field`; `None` removes the key.
    fn value(&self, field: Field) -> Option<FmValue> {
        let text = |t: &str| Some(FmValue::Text(t.to_owned()));
        match field {
            Field::Name | Field::Prompt => None,
            Field::Description => Some(self.description.trim())
                .filter(|d| !d.is_empty())
                .and_then(text),
            Field::Model => text(self.model.as_deref().unwrap_or("inherit")),
            Field::Thinking => text(self.thinking.as_deref().unwrap_or("inherit")),
            Field::Tools => match &self.tools {
                None => text("all"),
                Some(tools) => Some(FmValue::List(tools.clone())),
            },
            Field::Disallowed => Some(FmValue::List(self.disallowed.clone())),
            Field::MaxTurns => Some(self.max_turns.trim())
                .filter(|t| !t.is_empty())
                .and_then(text),
            Field::PromptMode => text(match self.prompt_mode {
                PromptMode::Append => "append",
                PromptMode::Replace => "replace",
            }),
            Field::Exposure => self.exposure.and_then(|e| text(e.as_str())),
        }
    }

    pub fn key(field: Field) -> &'static str {
        match field {
            Field::Name => "name",
            Field::Description => "description",
            Field::Model => "model",
            Field::Thinking => "thinking",
            Field::Tools => "tools",
            Field::Disallowed => "disallowedTools",
            Field::MaxTurns => "maxTurns",
            Field::PromptMode => "systemPromptMode",
            Field::Exposure => "exposure",
            Field::Prompt => "",
        }
    }

    /// A whole agent file with every field set, for new agents and copies.
    pub fn file(&self, name: &str) -> Frontmatter {
        let mut fm = Frontmatter {
            fields: vec![("name".into(), FmValue::Text(name.into()))],
            body: self.prompt.trim().to_owned(),
        };
        let order = [
            Field::Description,
            Field::Tools,
            Field::Disallowed,
            Field::Model,
            Field::Thinking,
            Field::MaxTurns,
            Field::PromptMode,
            Field::Exposure,
        ];
        for field in &order {
            let skip = match field {
                Field::Disallowed => self.disallowed.is_empty(),
                Field::Thinking => self.thinking.is_none(),
                Field::PromptMode => self.prompt_mode == PromptMode::Append,
                _ => false,
            };
            if !skip && let Some(value) = self.value(*field) {
                fm.set(Self::key(*field), Some(value));
            }
        }
        fm
    }
}

/// A validation finding; errors block the save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub error: bool,
    pub field: Field,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub name: String,
    pub values: Values,
    original: Values,
}

impl Draft {
    pub fn new(agent: &AgentDef) -> Self {
        let values = Values::of(agent);
        Self {
            name: agent.name.clone(),
            original: values.clone(),
            values,
        }
    }

    pub fn changed(&self) -> Vec<Field> {
        let differs = |f: &&Field| self.values.differs(&self.original, **f);
        FIELDS.iter().filter(differs).copied().collect()
    }

    pub fn dirty(&self) -> bool {
        !self.changed().is_empty()
    }

    /// Steps a choice field; `false` for other fields.
    pub fn cycle(&mut self, field: Field, forward: bool) -> bool {
        let step = |len: usize, at: usize| {
            if forward {
                (at + 1) % len
            } else {
                (at + len - 1) % len
            }
        };
        let v = &mut self.values;
        match field {
            Field::Thinking => {
                let current = v.thinking.as_deref().unwrap_or("inherit");
                let at = THINKING.iter().position(|t| *t == current).unwrap_or(0);
                let next = THINKING[step(THINKING.len(), at)];
                v.thinking = (next != "inherit").then(|| next.to_owned());
            }
            Field::PromptMode => {
                v.prompt_mode = match v.prompt_mode {
                    PromptMode::Append => PromptMode::Replace,
                    PromptMode::Replace => PromptMode::Append,
                }
            }
            Field::Exposure => {
                let order = [None, Some(Exposure::Direct), Some(Exposure::Deferred)];
                let at = order.iter().position(|e| *e == v.exposure).unwrap_or(0);
                v.exposure = order[step(order.len(), at)];
            }
            _ => return false,
        }
        true
    }

    /// Sets a field to its default (inherit, all tools, none); `false` when it has none.
    pub fn clear(&mut self, field: Field) -> bool {
        let v = &mut self.values;
        match field {
            Field::Model => v.model = None,
            Field::Thinking => v.thinking = None,
            Field::Tools => v.tools = None,
            Field::Disallowed => v.disallowed.clear(),
            Field::MaxTurns => v.max_turns.clear(),
            Field::PromptMode => v.prompt_mode = PromptMode::Append,
            Field::Exposure => v.exposure = None,
            _ => return false,
        }
        true
    }

    /// `known` is the tool names of this session; empty skips the tool check.
    pub fn problems(&self, known: &[String]) -> Vec<Problem> {
        let v = &self.values;
        let mut out = Vec::new();
        let mut add =
            |error: bool, field: Field, text: String| out.push(Problem { error, field, text });
        if v.description.trim().is_empty() {
            add(
                true,
                Field::Description,
                "Description is empty: the parent model reads it to pick the agent.".into(),
            );
        }
        if let Some(model) = &v.model
            && model
                .split_once('/')
                .is_none_or(|(p, id)| p.trim().is_empty() || id.trim().is_empty())
        {
            add(
                true,
                Field::Model,
                format!("Model {model:?} is not provider/id."),
            );
        }
        let turns = v.max_turns.trim();
        if !turns.is_empty() && turns.parse::<u32>().ok().is_none_or(|t| t == 0) {
            add(
                true,
                Field::MaxTurns,
                format!("Max turns {turns:?} is not a whole number above 0."),
            );
        }
        let unknown = |list: &[String]| -> Vec<String> {
            let hidden = |t: &String| t.starts_with("mcp__") || t == crate::messaging::TOOL;
            let missing = list.iter().filter(|t| !known.contains(t) && !hidden(t));
            if known.is_empty() {
                Vec::new()
            } else {
                missing.cloned().collect()
            }
        };
        match &v.tools {
            Some(tools) if tools.is_empty() => add(
                false,
                Field::Tools,
                "No tools: the agent can only answer from its prompt.".into(),
            ),
            Some(tools) if !unknown(tools).is_empty() => add(
                false,
                Field::Tools,
                format!(
                    "Unknown tools: {} (not in this session).",
                    unknown(tools).join(", ")
                ),
            ),
            _ => {}
        }
        if let Some(tools) = &v.tools
            && let Some(both) = tools.iter().find(|t| v.disallowed.contains(t))
        {
            add(
                false,
                Field::Disallowed,
                format!("{both} is in Tools and in Denied tools; it is denied."),
            );
        }
        if v.prompt.trim().is_empty() {
            let text = match v.prompt_mode {
                PromptMode::Replace => "The prompt is empty, so replace mode keeps pi's prompt.",
                PromptMode::Append => "The prompt is empty: the child gets no instructions.",
            };
            add(false, Field::Prompt, text.into());
        }
        out
    }

    /// Writes the changed fields into the agent's file `fm`.
    pub fn apply(&self, fm: &mut Frontmatter) {
        for field in self.changed() {
            if field == Field::Prompt {
                fm.body = self.values.prompt.trim().to_owned();
                continue;
            }
            let key = Values::key(field);
            let mut value = self.values.value(field);
            // A lower layer still sets the turns when this file has no key, so 0 clears them.
            if field == Field::MaxTurns && value.is_none() && fm.get(key).is_none() {
                value = Some(FmValue::Text("0".into()));
            }
            fm.set(key, value);
        }
    }

    pub fn saved(&mut self) {
        self.original = self.values.clone();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    ReadOnly,
    Worker,
    Reviewer,
}

pub const TEMPLATES: [(Template, &str, &str); 3] = [
    (
        Template::ReadOnly,
        "Read-only helper",
        "read, grep, find, ls; searches and reports",
    ),
    (
        Template::Worker,
        "Worker",
        "every tool; makes changes and runs checks",
    ),
    (
        Template::Reviewer,
        "Reviewer",
        "read-only; reviews a change and recommends",
    ),
];

pub fn template(kind: Template, name: &str) -> Values {
    let read_only = Some(["read", "grep", "find", "ls"].map(String::from).to_vec());
    let (tools, description, job) = match kind {
        Template::ReadOnly => (
            read_only,
            "Read-only helper. Say when the parent should use it.",
            "You search and read files to answer one question. You do not change files.",
        ),
        Template::Worker => (
            None,
            "Worker with every tool. Say when the parent should use it.",
            "You complete one task with the full tool set, then run the checks that cover it.",
        ),
        Template::Reviewer => (
            read_only,
            "Reviewer that checks a plan or change and recommends. Say when to use it.",
            "You review a plan or change. You do not change files.",
        ),
    };
    let prompt = format!(
        "You are the {name} subagent. {job}\n\n- Describe the method step by step.\n- End with the shape of the final answer the parent gets."
    );
    Values {
        description: description.into(),
        model: None,
        thinking: None,
        tools,
        disallowed: Vec::new(),
        max_turns: String::new(),
        prompt_mode: PromptMode::Append,
        exposure: None,
        prompt,
    }
}
