use std::path::PathBuf;

use serde::Serialize;

use crate::frontmatter::{FmValue, Frontmatter};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Builtin,
    Global,
    Project,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Builtin => "built-in",
            Source::Global => "global",
            Source::Project => "project",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptMode {
    #[default]
    Append,
    Replace,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDef {
    pub name: String,
    pub description: String,
    pub prompt: String,
    /// `None` gives the child every tool except `subagent`.
    pub tools: Option<Vec<String>>,
    /// `provider/id`; `None` inherits the parent session's model and thinking level.
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub max_turns: Option<u32>,
    pub prompt_mode: PromptMode,
    pub source: Source,
    pub builtin: bool,
    /// Override files that changed this agent, lowest precedence first.
    pub files: Vec<PathBuf>,
}

impl AgentDef {
    pub fn overridden(&self) -> bool {
        self.builtin && !self.files.is_empty()
    }

    pub fn tools_label(&self) -> String {
        match &self.tools {
            Some(tools) if tools.is_empty() => "none".into(),
            Some(tools) => tools.join(", "),
            None => "all".into(),
        }
    }

    pub fn model_label(&self) -> String {
        let model = self.model.as_deref().unwrap_or("session model");
        match &self.thinking {
            Some(thinking) => format!("{model}, thinking {thinking}"),
            None if self.model.is_none() => format!("{model} and thinking"),
            None => model.to_owned(),
        }
    }
}

/// The fields of one agent file. `None` keeps the value from lower layers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentPatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub prompt: Option<String>,
    pub tools: Option<Option<Vec<String>>>,
    pub model: Option<Option<String>>,
    pub thinking: Option<Option<String>>,
    pub max_turns: Option<u32>,
    pub prompt_mode: Option<PromptMode>,
}

impl AgentPatch {
    pub fn from_frontmatter(fm: &Frontmatter) -> Self {
        let tools = fm.get("tools").map(|value| {
            let names = value.as_list();
            let all = names.iter().any(|name| name == "*" || name == "all");
            (!all).then(|| map_tools(&names))
        });
        Self {
            name: fm.text("name"),
            description: fm.text("description"),
            prompt: Some(fm.body.clone()).filter(|body| !body.is_empty()),
            tools,
            model: fm.get("model").map(|value| model_value(&value.as_text())),
            thinking: fm.get("thinking").map(|value| {
                let level = value.as_text().trim().to_ascii_lowercase();
                Some(level).filter(|level| !level.is_empty() && level != "inherit")
            }),
            max_turns: fm
                .text("maxTurns")
                .and_then(|turns| turns.trim().parse().ok()),
            prompt_mode: fm.text("systemPromptMode").map(|mode| {
                if mode.trim().eq_ignore_ascii_case("replace") {
                    PromptMode::Replace
                } else {
                    PromptMode::Append
                }
            }),
        }
    }

    pub fn apply(self, agent: &mut AgentDef) {
        if let Some(description) = self.description {
            agent.description = description;
        }
        if let Some(prompt) = self.prompt {
            agent.prompt = prompt;
        }
        if let Some(tools) = self.tools {
            agent.tools = tools;
        }
        if let Some(model) = self.model {
            agent.model = model;
        }
        if let Some(thinking) = self.thinking {
            agent.thinking = thinking;
        }
        if let Some(turns) = self.max_turns {
            agent.max_turns = Some(turns).filter(|turns| *turns > 0);
        }
        if let Some(mode) = self.prompt_mode {
            agent.prompt_mode = mode;
        }
    }
}

/// `inherit` and the Claude aliases `sonnet`, `opus`, `haiku` use the session model.
pub fn model_value(raw: &str) -> Option<String> {
    let model = raw.trim();
    let alias = ["", "inherit", "default", "sonnet", "opus", "haiku"];
    if alias.iter().any(|a| model.eq_ignore_ascii_case(a)) {
        None
    } else {
        Some(model.to_owned())
    }
}

/// Maps Claude Code tool names to pi tool names; other names pass through lowercased.
pub fn map_tools(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in names {
        let base = name.split('(').next().unwrap_or(name).trim();
        let mapped: String = match base {
            "Read" => "read".into(),
            "Write" => "write".into(),
            "Edit" | "MultiEdit" | "NotebookEdit" => "edit".into(),
            "Bash" | "BashOutput" => "bash".into(),
            "Grep" => "grep".into(),
            "Glob" => "find".into(),
            "LS" => "ls".into(),
            "TodoWrite" | "TodoRead" => "todo".into(),
            "Task" | "Agent" | "KillBash" | "KillShell" => continue,
            other if other.starts_with("mcp__") => other.into(),
            other => other.to_ascii_lowercase(),
        };
        if !mapped.is_empty() && !out.contains(&mapped) {
            out.push(mapped);
        }
    }
    out
}

/// The file text for an agent made from scratch in `/agents`.
pub fn new_agent_file(name: &str, description: &str, prompt: &str) -> Frontmatter {
    Frontmatter {
        fields: vec![
            ("name".into(), FmValue::Text(name.into())),
            ("description".into(), FmValue::Text(description.into())),
            ("tools".into(), FmValue::Text("read, grep, find, ls".into())),
            ("model".into(), FmValue::Text("inherit".into())),
        ],
        body: prompt.to_owned(),
    }
}
