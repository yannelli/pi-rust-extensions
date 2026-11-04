//! Workflow definitions. A workflow runs its steps in order; one agent session runs each step
//! and works through the step's tasks.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
/// The model value that makes the run dialog ask for a model.
pub const ASK: &str = "ask";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Workflow {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "is_default")]
    pub mode: Mode,
    #[serde(skip_serializing_if = "is_default")]
    pub context: ContextMode,
    /// Runs keep going after pi quits.
    #[serde(skip_serializing_if = "is_false")]
    pub background: bool,
    /// Defaults for the agent of every step.
    #[serde(skip_serializing_if = "is_default")]
    pub agent: Scope,
    #[serde(skip_serializing_if = "is_default")]
    pub planner: Planner,
    pub steps: Vec<Step>,
}

/// `fixed` runs the steps as written; `dynamic` lets a planner model write and revise the
/// remaining steps before each step, with the written steps as the starting plan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Fixed,
    Dynamic,
}

/// What a step's agent learns about earlier steps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextMode {
    /// The reports of the earlier steps.
    #[default]
    Handoff,
    /// The reports plus a shared notes file that every agent can add to.
    Memory,
    /// One conversation for all steps; pi compacts it when it grows.
    Session,
    /// Nothing but the step's own tasks.
    None,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Planner {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u32>,
}

/// Agent settings. `None` inherits: a step from the workflow `agent`, the workflow from the
/// pi session that starts the run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Scope {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagents: Option<Vec<SubagentRule>>,
    /// Values may reference the environment of pi as `${NAME}`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_cost: Option<f64>,
}

/// One subagent type a step may start. The first model without `*` and the first thinking
/// level are the defaults for calls that give none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct SubagentRule {
    pub agent: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub thinking: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Step {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextMode>,
    #[serde(skip_serializing_if = "is_default")]
    pub agent: Scope,
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Task {
    pub prompt: String,
    /// Runs the prompt as `/skill:<name> <prompt>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    /// Wait before the first run, for example `5m`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delay: Option<String>,
    /// Runs of the task; with `until`, the most runs before the goal counts as missed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat: Option<u32>,
    /// Wait between runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub every: Option<String>,
    /// A goal: the task runs again until the agent reports it met.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    /// Ask the user before the task runs.
    #[serde(skip_serializing_if = "is_false")]
    pub confirm: bool,
    /// A failure skips the rest of the task instead of stopping the workflow.
    #[serde(skip_serializing_if = "is_false")]
    pub optional: bool,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Fixed => "fixed",
            Mode::Dynamic => "dynamic",
        }
    }
}

impl ContextMode {
    pub const ALL: [ContextMode; 4] = [
        ContextMode::Handoff,
        ContextMode::Memory,
        ContextMode::Session,
        ContextMode::None,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ContextMode::Handoff => "handoff",
            ContextMode::Memory => "memory",
            ContextMode::Session => "session",
            ContextMode::None => "none",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            ContextMode::Handoff => "each agent gets the reports of the earlier steps",
            ContextMode::Memory => "reports plus a shared notes file that every agent adds to",
            ContextMode::Session => "all steps continue one conversation",
            ContextMode::None => "each agent sees only its own tasks",
        }
    }
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

fn is_false(value: &bool) -> bool {
    !value
}

const DEFAULT_UNTIL_RUNS: u32 = 10;
const DEFAULT_UNTIL_EVERY: Duration = Duration::from_secs(60);

impl Task {
    pub fn max_runs(&self) -> u32 {
        let default = if self.until.is_some() {
            DEFAULT_UNTIL_RUNS
        } else {
            1
        };
        self.repeat.unwrap_or(default).max(1)
    }

    pub fn every(&self) -> Duration {
        let default = if self.until.is_some() {
            DEFAULT_UNTIL_EVERY
        } else {
            Duration::ZERO
        };
        self.every
            .as_deref()
            .and_then(parse_duration)
            .unwrap_or(default)
    }

    pub fn delay(&self) -> Duration {
        self.delay
            .as_deref()
            .and_then(parse_duration)
            .unwrap_or_default()
    }
}

impl Scope {
    /// `self` over `base`: set fields win and `env` entries merge.
    pub fn over(&self, base: &Scope) -> Scope {
        let mut env = base.env.clone();
        env.extend(self.env.clone());
        Scope {
            model: self.model.clone().or_else(|| base.model.clone()),
            thinking: self.thinking.clone().or_else(|| base.thinking.clone()),
            tools: self.tools.clone().or_else(|| base.tools.clone()),
            skills: self.skills.clone().or_else(|| base.skills.clone()),
            mcp: self.mcp.clone().or_else(|| base.mcp.clone()),
            subagents: self.subagents.clone().or_else(|| base.subagents.clone()),
            env,
            system_prompt: self
                .system_prompt
                .clone()
                .or_else(|| base.system_prompt.clone()),
            max_turns: self.max_turns.or(base.max_turns),
            max_cost: self.max_cost.or(base.max_cost),
        }
    }
}

impl Workflow {
    /// The step's agent settings over the workflow defaults.
    pub fn scope(&self, step: &Step) -> Scope {
        step.agent.over(&self.agent)
    }

    pub fn context_of(&self, step: &Step) -> ContextMode {
        step.context.unwrap_or(self.context)
    }

    /// The `{{name}}` placeholders of all prompts, in order of first use. A dynamic workflow
    /// always has `goal`.
    pub fn inputs(&self) -> Vec<String> {
        let mut names = Vec::new();
        if self.mode == Mode::Dynamic {
            names.push("goal".to_owned());
        }
        let mut texts: Vec<&str> = vec![&self.planner.instructions];
        texts.extend(self.agent.system_prompt.as_deref());
        for step in &self.steps {
            texts.extend(step.agent.system_prompt.as_deref());
            for task in &step.tasks {
                texts.push(&task.prompt);
                texts.extend(task.until.as_deref());
            }
        }
        for text in texts {
            for name in placeholders(text) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
    }

    /// Problems that stop the workflow from running; empty when it is valid.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if slug(&self.name).is_empty() {
            problems.push("the workflow needs a name".to_owned());
        }
        if self.steps.is_empty() && self.mode == Mode::Fixed {
            problems.push("a fixed workflow needs at least one step".to_owned());
        }
        check_scope("workflow agent", &self.agent, &mut problems);
        if let Some(model) = &self.planner.model {
            check_model("planner", model, &mut problems);
        }
        let mut names: Vec<String> = Vec::new();
        for (index, step) in self.steps.iter().enumerate() {
            let label = format!("step {} ({})", index + 1, step.name);
            problems.extend(step_problems(step, &label));
            let name = slug(&step.name);
            if !name.is_empty() && names.contains(&name) {
                problems.push(format!("{label}: another step has the same name"));
            }
            names.push(name);
        }
        problems
    }
}

pub fn step_problems(step: &Step, label: &str) -> Vec<String> {
    let mut problems = Vec::new();
    if slug(&step.name).is_empty() {
        problems.push(format!("{label}: the step needs a name"));
    }
    if step.tasks.is_empty() {
        problems.push(format!("{label}: the step needs at least one task"));
    }
    check_scope(label, &step.agent, &mut problems);
    for (index, task) in step.tasks.iter().enumerate() {
        let label = format!("{label} task {}", index + 1);
        if task.prompt.trim().is_empty() && task.skill.is_none() {
            problems.push(format!("{label}: the task needs a prompt or a skill"));
        }
        for (field, value) in [("delay", &task.delay), ("every", &task.every)] {
            if value
                .as_deref()
                .is_some_and(|v| parse_duration(v).is_none())
            {
                problems.push(format!(
                    "{label}: {field} is not a duration such as 30s, 5m, or 1h30m"
                ));
            }
        }
        if task.repeat == Some(0) {
            problems.push(format!("{label}: repeat must be at least 1"));
        }
        if task.skill.as_deref().is_some_and(|s| s.trim().is_empty()) {
            problems.push(format!("{label}: the skill name is empty"));
        }
    }
    problems
}

fn check_scope(label: &str, scope: &Scope, problems: &mut Vec<String>) {
    if let Some(model) = &scope.model {
        check_model(label, model, problems);
    }
    if let Some(level) = &scope.thinking
        && !THINKING_LEVELS.contains(&level.as_str())
    {
        problems.push(format!(
            "{label}: thinking must be one of {}",
            THINKING_LEVELS.join(", ")
        ));
    }
    if scope.max_cost.is_some_and(|cost| cost <= 0.0) {
        problems.push(format!("{label}: maxCost must be above 0"));
    }
    if scope.max_turns == Some(0) {
        problems.push(format!("{label}: maxTurns must be at least 1"));
    }
    for key in scope.env.keys() {
        let valid = key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && key.chars().next().is_some_and(|c| !c.is_ascii_digit());
        if !valid {
            problems.push(format!(
                "{label}: env name {key:?} is not a valid variable name"
            ));
        }
    }
    for rule in scope.subagents.iter().flatten() {
        if rule.agent.trim().is_empty() {
            problems.push(format!("{label}: a subagent rule needs an agent"));
        }
        for level in &rule.thinking {
            if !THINKING_LEVELS.contains(&level.as_str()) {
                problems.push(format!(
                    "{label}: subagent {} thinking {level} is unknown",
                    rule.agent
                ));
            }
        }
    }
}

fn check_model(label: &str, model: &str, problems: &mut Vec<String>) {
    let valid = model == ASK
        || model
            .split_once('/')
            .is_some_and(|(p, id)| !p.is_empty() && !id.is_empty());
    if !valid {
        problems.push(format!(
            "{label}: model {model:?} must be provider/id or \"ask\""
        ));
    }
}

/// `{{ name }}` placeholders in `text`.
pub fn placeholders(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let name = after[..end].trim();
        if !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            names.push(name.to_owned());
        }
        rest = &after[end + 2..];
    }
    names
}

/// Replaces `{{ name }}` with the input of that name; unknown names stay.
pub fn fill(text: &str, inputs: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            rest = &rest[start..];
            break;
        };
        match inputs.get(after[..end].trim()) {
            Some(value) => out.push_str(value),
            None => out.push_str(&rest[start..start + 2 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// `30s`, `5m`, `1h30m`, `2d`, or plain seconds.
pub fn parse_duration(text: &str) -> Option<Duration> {
    let text = text.trim();
    if let Ok(seconds) = text.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let mut total = 0u64;
    let mut number = String::new();
    for c in text.chars() {
        if c.is_ascii_digit() {
            number.push(c);
            continue;
        }
        let unit = match c {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86_400,
            _ => return None,
        };
        total += number.parse::<u64>().ok()? * unit;
        number.clear();
    }
    (number.is_empty() && !text.is_empty()).then(|| Duration::from_secs(total))
}

/// `1h30m`, `5m`, `45s`, `0s`.
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    let mut out = String::new();
    if h > 0 {
        out.push_str(&format!("{h}h"));
    }
    if m > 0 {
        out.push_str(&format!("{m}m"));
    }
    if s > 0 || out.is_empty() {
        out.push_str(&format!("{s}s"));
    }
    out
}

/// Lowercase letters, digits, and single dashes: the file and session name of a workflow or step.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// `*` matches any run of characters.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || text.len() < first.len() + last.len() || !text.ends_with(last) {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn durations_parse_and_format() {
        assert_eq!(parse_duration("90"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_duration("1h30m"), Some(Duration::from_secs(5400)));
        assert_eq!(parse_duration("2x"), None);
        assert_eq!(parse_duration("5m3"), None);
        assert_eq!(parse_duration(""), None);
        assert_eq!(format_duration(Duration::from_secs(5400)), "1h30m");
        assert_eq!(format_duration(Duration::from_secs(45)), "45s");
        assert_eq!(format_duration(Duration::ZERO), "0s");
    }

    #[test]
    fn inputs_and_fill() {
        let workflow: Workflow = serde_json::from_value(json!({
            "name": "ship",
            "steps": [{ "name": "a", "tasks": [{ "prompt": "Do {{task}} in {{ repo }}", "until": "{{task}} merged" }] }]
        }))
        .unwrap();
        assert_eq!(workflow.inputs(), ["task", "repo"]);
        let inputs = BTreeMap::from([("task".to_owned(), "X".to_owned())]);
        assert_eq!(
            fill("Do {{task}} in {{ repo }} {{", &inputs),
            "Do X in {{ repo }} {{"
        );
    }

    #[test]
    fn unknown_fields_are_errors() {
        let error = serde_json::from_value::<Workflow>(
            json!({ "name": "a", "steps": [{ "name": "s", "tasks": [], "modle": "x" }] }),
        );
        assert!(error.unwrap_err().to_string().contains("modle"));
    }

    #[test]
    fn problems_name_the_step_and_field() {
        let workflow: Workflow = serde_json::from_value(json!({
            "name": "ship",
            "agent": { "thinking": "huge" },
            "steps": [
                { "name": "a", "agent": { "model": "nope" }, "tasks": [{ "prompt": "x", "every": "soon" }] },
                { "name": "A", "tasks": [] }
            ]
        }))
        .unwrap();
        let problems = workflow.problems().join("\n");
        assert!(problems.contains("workflow agent: thinking"), "{problems}");
        assert!(
            problems.contains("step 1 (a): model \"nope\""),
            "{problems}"
        );
        assert!(problems.contains("step 1 (a) task 1: every"), "{problems}");
        assert!(
            problems.contains("step 2 (A): the step needs at least one task"),
            "{problems}"
        );
        assert!(
            problems.contains("step 2 (A): another step has the same name"),
            "{problems}"
        );
    }

    #[test]
    fn scope_merges_over_defaults() {
        let base = Scope {
            model: Some("a/b".into()),
            env: BTreeMap::from([("A".into(), "1".into()), ("B".into(), "1".into())]),
            ..Scope::default()
        };
        let step = Scope {
            thinking: Some("low".into()),
            env: BTreeMap::from([("B".into(), "2".into())]),
            ..Scope::default()
        };
        let merged = step.over(&base);
        assert_eq!(merged.model.as_deref(), Some("a/b"));
        assert_eq!(merged.thinking.as_deref(), Some("low"));
        assert_eq!(merged.env["B"], "2");
        assert_eq!(merged.env["A"], "1");
    }

    #[test]
    fn task_runs_and_waits() {
        let task = Task {
            until: Some("green".into()),
            ..Task::default()
        };
        assert_eq!(
            (task.max_runs(), task.every()),
            (10, Duration::from_secs(60))
        );
        let task = Task {
            repeat: Some(3),
            every: Some("5m".into()),
            ..Task::default()
        };
        assert_eq!(
            (task.max_runs(), task.every()),
            (3, Duration::from_secs(300))
        );
    }

    #[test]
    fn slugs_and_globs() {
        assert_eq!(slug("  Ship It! (v2) "), "ship-it-v2");
        assert!(glob_match("anthropic/*", "anthropic/claude-haiku-4-5"));
        assert!(glob_match("*haiku*", "anthropic/claude-haiku-4-5"));
        assert!(!glob_match("openai/*", "anthropic/x"));
        assert!(glob_match("a/b", "a/b"));
    }

    #[test]
    fn files_skip_defaults() {
        let workflow = Workflow {
            name: "w".into(),
            steps: vec![Step {
                name: "s".into(),
                tasks: vec![Task {
                    prompt: "p".into(),
                    ..Task::default()
                }],
                ..Step::default()
            }],
            ..Workflow::default()
        };
        assert_eq!(
            serde_json::to_value(&workflow).unwrap(),
            json!({ "name": "w", "steps": [{ "name": "s", "tasks": [{ "prompt": "p" }] }] })
        );
    }
}
