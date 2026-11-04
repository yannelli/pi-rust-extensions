//! The form fields of the editor: what each shows, its help line, and how it is edited.

use std::collections::BTreeMap;

use pi_ext_sdk::InputOptions;
use pi_ext_sdk::prelude::*;

use crate::model::{
    ASK, ContextMode, Mode, Scope, SubagentRule, THINKING_LEVELS, Task, Workflow, parse_duration,
    slug,
};
use crate::ui::picker::{self, Item, Picked};
use crate::ui::theme::Paint;
use crate::ui::{Screen, Ui, one_line};

/// A row of the editor tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    Settings,
    Step(usize),
    Task(usize, usize),
    AddStep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    Description,
    Mode,
    Context,
    Background,
    Model,
    Thinking,
    Tools,
    Skills,
    Mcp,
    Subagents,
    Env,
    SystemPrompt,
    MaxTurns,
    MaxCost,
    PlannerRules,
    PlannerModel,
    MaxSteps,
    Prompt,
    Skill,
    Delay,
    Repeat,
    Every,
    Until,
    Confirm,
    Optional,
}

pub enum Edit {
    Unchanged,
    Changed,
    Invalid(String),
}

const SCOPE: [Field; 10] = [
    Field::Model,
    Field::Thinking,
    Field::Tools,
    Field::Skills,
    Field::Mcp,
    Field::Subagents,
    Field::Env,
    Field::SystemPrompt,
    Field::MaxTurns,
    Field::MaxCost,
];

pub fn fields(node: Node, workflow: &Workflow) -> Vec<Field> {
    use Field::*;
    match node {
        Node::Settings => {
            let mut fields = vec![Name, Description, Mode, Context, Background];
            fields.extend(SCOPE);
            if workflow.mode == crate::model::Mode::Dynamic {
                fields.extend([PlannerRules, PlannerModel, MaxSteps]);
            }
            fields
        }
        Node::Step(_) => [Name, Context].into_iter().chain(SCOPE).collect(),
        Node::Task(..) => vec![
            Prompt, Skill, Delay, Repeat, Every, Until, Confirm, Optional,
        ],
        Node::AddStep => Vec::new(),
    }
}

pub fn label(field: Field) -> &'static str {
    match field {
        Field::Name => "Name",
        Field::Description => "Description",
        Field::Mode => "Mode",
        Field::Context => "Context",
        Field::Background => "Background",
        Field::Model => "Model",
        Field::Thinking => "Thinking",
        Field::Tools => "Tools",
        Field::Skills => "Skills",
        Field::Mcp => "MCP servers",
        Field::Subagents => "Subagents",
        Field::Env => "Environment",
        Field::SystemPrompt => "System prompt",
        Field::MaxTurns => "Max turns",
        Field::MaxCost => "Max cost",
        Field::PlannerRules => "Planner rules",
        Field::PlannerModel => "Planner model",
        Field::MaxSteps => "Max steps",
        Field::Prompt => "Prompt",
        Field::Skill => "Skill",
        Field::Delay => "Delay",
        Field::Repeat => "Repeat",
        Field::Every => "Every",
        Field::Until => "Until",
        Field::Confirm => "Ask first",
        Field::Optional => "Optional",
    }
}

pub fn help(field: Field) -> &'static str {
    match field {
        Field::Name => "Lowercase letters, digits, and dashes.",
        Field::Description => "What the workflow does. Every agent sees it.",
        Field::Mode => {
            "Fixed runs the steps as written. Dynamic lets a planner model revise the remaining steps before each step."
        }
        Field::Context => "What each agent learns about the earlier steps.",
        Field::Background => {
            "Runs keep going after pi quits. b in the run view switches a running run."
        }
        Field::Model => "provider/id, or ask to choose when the run starts.",
        Field::Thinking => "The thinking level of the agent.",
        Field::Tools => "The tools the agent may use.",
        Field::Skills => "The skills the agent loads. Skills that tasks run always load.",
        Field::Mcp => "The MCP servers the agent may use.",
        Field::Subagents => {
            "The subagent types the agent may start, with their models and thinking levels."
        }
        Field::Env => {
            "Variables for the agent. ${NAME} reads pi's environment, so secrets stay out of the file."
        }
        Field::SystemPrompt => "Text added to the agent's system prompt.",
        Field::MaxTurns => "Stop a task run after this many agent turns.",
        Field::MaxCost => "Stop the agent when its session costs this many US dollars.",
        Field::PlannerRules => "Rules for the planner, such as which steps to always include.",
        Field::PlannerModel => "The model that plans. Default: the session model.",
        Field::MaxSteps => "The most steps the planner may plan. Default: 12.",
        Field::Prompt => "What to do. {{name}} placeholders become inputs when the run starts.",
        Field::Skill => "Run the prompt as /skill:<name>.",
        Field::Delay => "Wait before the first run, such as 30s, 5m, or 1h30m.",
        Field::Repeat => {
            "How many runs. With a goal: the most runs before the goal counts as missed."
        }
        Field::Every => "Wait between runs.",
        Field::Until => "A goal. The task runs again until the agent reports it met.",
        Field::Confirm => "Ask you before the task runs. Use it for merges to main and deploys.",
        Field::Optional => "When the task fails, skip it, and continue the workflow.",
    }
}

fn scope(node: Node, workflow: &Workflow) -> Option<&Scope> {
    match node {
        Node::Settings => Some(&workflow.agent),
        Node::Step(i) => Some(&workflow.steps[i].agent),
        _ => None,
    }
}

fn scope_mut(node: Node, workflow: &mut Workflow) -> Option<&mut Scope> {
    match node {
        Node::Settings => Some(&mut workflow.agent),
        Node::Step(i) => Some(&mut workflow.steps[i].agent),
        _ => None,
    }
}

fn task_mut(node: Node, workflow: &mut Workflow) -> Option<&mut Task> {
    match node {
        Node::Task(s, t) => Some(&mut workflow.steps[s].tasks[t]),
        _ => None,
    }
}

/// The value of a field as the form shows it.
pub fn value(field: Field, node: Node, workflow: &Workflow, paint: &Paint) -> String {
    let none = |text: &str| paint.dim(text);
    if let Node::Task(s, t) = node {
        let task = &workflow.steps[s].tasks[t];
        let text = |value: &Option<String>, empty: &str| {
            value
                .as_deref()
                .map_or_else(|| none(empty), |v| one_line(v, 80))
        };
        let check = |on: bool| {
            if on {
                paint.accent("[✓] yes")
            } else {
                paint.dim("[ ] no")
            }
        };
        return match field {
            Field::Prompt if task.prompt.trim().is_empty() => paint.fg("warning", "empty"),
            Field::Prompt => one_line(&task.prompt, 80),
            Field::Skill => text(&task.skill, "none"),
            Field::Delay => text(&task.delay, "none"),
            Field::Repeat => task.repeat.map_or_else(
                || none(&format!("{} run(s)", task.max_runs())),
                |n| n.to_string(),
            ),
            Field::Every if task.until.is_none() => text(&task.every, "none"),
            Field::Every => text(&task.every, &crate::model::format_duration(task.every())),
            Field::Until => text(&task.until, "none"),
            Field::Confirm => check(task.confirm),
            Field::Optional => check(task.optional),
            _ => String::new(),
        };
    }
    match field {
        Field::Name => match node {
            Node::Step(i) => workflow.steps[i].name.clone(),
            _ if workflow.name.is_empty() => paint.fg("warning", "needs a name"),
            _ => workflow.name.clone(),
        },
        Field::Description if workflow.description.is_empty() => none("none"),
        Field::Description => one_line(&workflow.description, 80),
        Field::Mode => workflow.mode.label().to_owned(),
        Field::Background if workflow.background => paint.accent("[✓] yes"),
        Field::Background => paint.dim("[ ] no · stops when pi quits"),
        Field::Context => match node {
            Node::Step(i) => match workflow.steps[i].context {
                Some(context) => context.label().to_owned(),
                None => none(&format!("inherit · {}", workflow.context.label())),
            },
            _ => format!(
                "{} {}",
                workflow.context.label(),
                paint.dim(&format!("· {}", workflow.context.describe()))
            ),
        },
        Field::PlannerRules if workflow.planner.instructions.is_empty() => none("none"),
        Field::PlannerRules => one_line(&workflow.planner.instructions, 80),
        Field::PlannerModel => workflow
            .planner
            .model
            .clone()
            .unwrap_or_else(|| none("session model")),
        Field::MaxSteps => workflow
            .planner
            .max_steps
            .map_or_else(|| none("12"), |n| n.to_string()),
        _ => {
            let Some(own) = scope(node, workflow) else {
                return String::new();
            };
            let base = matches!(node, Node::Step(_)).then_some(&workflow.agent);
            match scope_text(field, own) {
                Some(text) => text,
                None => {
                    let from = base
                        .and_then(|b| scope_text(field, b))
                        .unwrap_or_else(|| unset_text(field).to_owned());
                    none(&format!("inherit · {from}"))
                }
            }
        }
    }
}

fn scope_text(field: Field, scope: &Scope) -> Option<String> {
    let list = |items: &Option<Vec<String>>| {
        items.as_ref().map(|v| {
            if v.is_empty() {
                "none".to_owned()
            } else {
                v.join(", ")
            }
        })
    };
    match field {
        Field::Model => scope.model.clone(),
        Field::Thinking => scope.thinking.clone(),
        Field::Tools => list(&scope.tools),
        Field::Skills => list(&scope.skills),
        Field::Mcp => list(&scope.mcp),
        Field::Subagents => scope.subagents.as_ref().map(|rules| {
            if rules.is_empty() {
                return "none".to_owned();
            }
            let rules: Vec<String> = rules.iter().map(rule_text).collect();
            rules.join("; ")
        }),
        Field::Env => (!scope.env.is_empty())
            .then(|| scope.env.keys().cloned().collect::<Vec<_>>().join(", ")),
        Field::SystemPrompt => scope.system_prompt.as_deref().map(|s| one_line(s, 60)),
        Field::MaxTurns => scope.max_turns.map(|n| n.to_string()),
        Field::MaxCost => scope.max_cost.map(|c| format!("${c:.2}")),
        _ => None,
    }
}

fn unset_text(field: Field) -> &'static str {
    match field {
        Field::Model | Field::Thinking => "session",
        Field::Tools => "pi defaults",
        Field::Skills | Field::Mcp => "all",
        Field::Subagents => "no limits",
        Field::MaxTurns | Field::MaxCost => "no limit",
        _ => "none",
    }
}

fn rule_text(rule: &SubagentRule) -> String {
    let mut limits: Vec<String> = rule
        .models
        .iter()
        .map(|m| crate::ui::short_model(m).to_owned())
        .collect();
    limits.extend(rule.thinking.iter().cloned());
    if limits.is_empty() {
        rule.agent.clone()
    } else {
        format!("{} ({})", rule.agent, limits.join(", "))
    }
}

/// Clears a field to its default; `false` when the field has no default.
pub fn reset(field: Field, node: Node, workflow: &mut Workflow) -> bool {
    if let Some(task) = task_mut(node, workflow) {
        match field {
            Field::Skill => task.skill = None,
            Field::Delay => task.delay = None,
            Field::Repeat => task.repeat = None,
            Field::Every => task.every = None,
            Field::Until => task.until = None,
            Field::Confirm => task.confirm = false,
            Field::Optional => task.optional = false,
            _ => return false,
        }
        return true;
    }
    match (field, node) {
        (Field::Context, Node::Step(i)) => workflow.steps[i].context = None,
        (Field::Description, _) => workflow.description.clear(),
        (Field::Background, _) => workflow.background = false,
        (Field::PlannerRules, _) => workflow.planner.instructions.clear(),
        (Field::PlannerModel, _) => workflow.planner.model = None,
        (Field::MaxSteps, _) => workflow.planner.max_steps = None,
        _ => {
            let Some(scope) = scope_mut(node, workflow) else {
                return false;
            };
            match field {
                Field::Model => scope.model = None,
                Field::Thinking => scope.thinking = None,
                Field::Tools => scope.tools = None,
                Field::Skills => scope.skills = None,
                Field::Mcp => scope.mcp = None,
                Field::Subagents => scope.subagents = None,
                Field::Env => scope.env.clear(),
                Field::SystemPrompt => scope.system_prompt = None,
                Field::MaxTurns => scope.max_turns = None,
                Field::MaxCost => scope.max_cost = None,
                _ => return false,
            }
        }
    }
    true
}

/// Edits a field with pi's dialogs or the panel picker.
pub async fn edit(
    field: Field,
    node: Node,
    workflow: &mut Workflow,
    ui: &Ui,
    screen: &mut Screen,
) -> Result<Edit> {
    if let Node::Task(s, t) = node {
        return edit_task(field, &mut workflow.steps[s].tasks[t], ui, screen).await;
    }
    let host = &ui.host;
    let changed = match field {
        Field::Name => {
            let current = match node {
                Node::Step(i) => workflow.steps[i].name.clone(),
                _ => workflow.name.clone(),
            };
            let Some(name) = text(ui, "Name", &current).await? else {
                return Ok(Edit::Unchanged);
            };
            let name = if matches!(node, Node::Step(_)) {
                name.trim().to_owned()
            } else {
                slug(&name)
            };
            if name.is_empty() {
                return Ok(Edit::Invalid("The name cannot be empty.".into()));
            }
            match node {
                Node::Step(i) => workflow.steps[i].name = name,
                _ => workflow.name = name,
            }
            true
        }
        Field::Description => {
            long_text(ui, "Description of the workflow", &mut workflow.description).await?
        }
        Field::Mode => {
            let options = [
                "fixed · run the steps as written",
                "dynamic · a planner writes the remaining steps before each step",
            ];
            let Some(choice) = host.select("Mode", &options).await? else {
                return Ok(Edit::Unchanged);
            };
            workflow.mode = if choice.starts_with("dynamic") {
                Mode::Dynamic
            } else {
                Mode::Fixed
            };
            true
        }
        Field::Context => {
            let step = matches!(node, Node::Step(_));
            let mut options: Vec<String> = Vec::new();
            if step {
                options.push(format!("inherit · {}", workflow.context.label()));
            }
            options.extend(
                ContextMode::ALL
                    .iter()
                    .map(|c| format!("{} · {}", c.label(), c.describe())),
            );
            let Some(choice) = host.select("Context", &options).await? else {
                return Ok(Edit::Unchanged);
            };
            let context = ContextMode::ALL
                .into_iter()
                .find(|c| choice.starts_with(c.label()));
            match (node, context) {
                (Node::Step(i), context) => workflow.steps[i].context = context,
                (_, Some(context)) => workflow.context = context,
                _ => {}
            }
            true
        }
        Field::Background => {
            workflow.background = !workflow.background;
            true
        }
        Field::PlannerRules => {
            long_text(
                ui,
                "Rules for the planner",
                &mut workflow.planner.instructions,
            )
            .await?
        }
        Field::PlannerModel => {
            match choose_model(ui, screen, "Planner model", false, "session model").await? {
                Some(model) => {
                    workflow.planner.model = model;
                    true
                }
                None => false,
            }
        }
        Field::MaxSteps => {
            match number(ui, "Max steps (empty for 12)", workflow.planner.max_steps).await? {
                Ok(value) => {
                    workflow.planner.max_steps = value;
                    true
                }
                Err(edit) => return Ok(edit),
            }
        }
        _ => {
            let Some(scope) = scope_mut(node, workflow) else {
                return Ok(Edit::Unchanged);
            };
            return edit_scope(field, scope, ui, screen).await;
        }
    };
    Ok(if changed {
        Edit::Changed
    } else {
        Edit::Unchanged
    })
}

async fn edit_scope(field: Field, scope: &mut Scope, ui: &Ui, screen: &mut Screen) -> Result<Edit> {
    let host = &ui.host;
    let changed = match field {
        Field::Model => match choose_model(
            ui,
            screen,
            "Model",
            true,
            "the workflow default or the session model",
        )
        .await?
        {
            Some(model) => {
                scope.model = model;
                true
            }
            None => false,
        },
        Field::Thinking => {
            let mut options = vec!["inherit"];
            options.extend(THINKING_LEVELS);
            let Some(choice) = host.select("Thinking level", &options).await? else {
                return Ok(Edit::Unchanged);
            };
            scope.thinking = (choice != "inherit").then_some(choice);
            true
        }
        Field::Tools => {
            let items: Vec<Item> = host
                .all_tools()
                .await
                .unwrap_or_default()
                .iter()
                .filter_map(|t| {
                    let name = t["name"].as_str()?;
                    let hidden = name.starts_with("mcp__")
                        || name == "workflow"
                        || name == crate::policy::MEMORY_TOOL;
                    (!hidden).then(|| Item::new(name, t["description"].as_str().unwrap_or("")))
                })
                .collect();
            let picked = picker::pick(
                ui,
                screen,
                "Tools",
                items,
                scope.tools.as_deref(),
                "pi defaults",
            )
            .await?;
            set_list(&mut scope.tools, picked)
        }
        Field::Skills => {
            let notes: BTreeMap<String, String> = host
                .all_commands()
                .await
                .unwrap_or_default()
                .iter()
                .filter_map(|c| {
                    let name = c["name"].as_str()?.strip_prefix("skill:")?;
                    Some((
                        name.to_owned(),
                        c["description"].as_str().unwrap_or("").to_owned(),
                    ))
                })
                .collect();
            let items = notes
                .into_iter()
                .map(|(name, note)| Item::new(name, note))
                .collect();
            let picked =
                picker::pick(ui, screen, "Skills", items, scope.skills.as_deref(), "all").await?;
            set_list(&mut scope.skills, picked)
        }
        Field::Mcp => {
            let catalog = ui.app.catalog(host).await;
            let items = catalog
                .mcp_servers
                .into_iter()
                .map(|s| Item::new(s, ""))
                .collect();
            let picked = picker::pick(
                ui,
                screen,
                "MCP servers",
                items,
                scope.mcp.as_deref(),
                "all",
            )
            .await?;
            set_list(&mut scope.mcp, picked)
        }
        Field::Subagents => match subagents(ui, screen, scope.subagents.as_deref()).await? {
            Some(rules) => {
                scope.subagents = rules;
                true
            }
            None => false,
        },
        Field::Env => {
            let current: Vec<String> = scope.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
            let prefill = format!("{ENV_HEADER}\n{}", current.join("\n"));
            let Some(text) = host.editor("Environment", Some(&prefill)).await? else {
                return Ok(Edit::Unchanged);
            };
            match parse_env(&text) {
                Ok(env) => {
                    scope.env = env;
                    true
                }
                Err(err) => return Ok(Edit::Invalid(err)),
            }
        }
        Field::SystemPrompt => {
            let mut text = scope.system_prompt.clone().unwrap_or_default();
            let changed = long_text(ui, "Text added to the system prompt", &mut text).await?;
            if changed {
                scope.system_prompt = (!text.trim().is_empty()).then_some(text);
            }
            changed
        }
        Field::MaxTurns => {
            match number(ui, "Max turns (empty for no limit)", scope.max_turns).await? {
                Ok(value) => {
                    scope.max_turns = value;
                    true
                }
                Err(edit) => return Ok(edit),
            }
        }
        Field::MaxCost => {
            let current = scope.max_cost.map(|c| c.to_string()).unwrap_or_default();
            let Some(text) =
                text(ui, "Max cost in US dollars (empty for no limit)", &current).await?
            else {
                return Ok(Edit::Unchanged);
            };
            let text = text.trim().trim_start_matches('$');
            if text.is_empty() {
                scope.max_cost = None;
            } else {
                match text.parse::<f64>() {
                    Ok(cost) if cost > 0.0 => scope.max_cost = Some(cost),
                    _ => return Ok(Edit::Invalid(format!("{text} is not an amount above 0."))),
                }
            }
            true
        }
        _ => false,
    };
    Ok(if changed {
        Edit::Changed
    } else {
        Edit::Unchanged
    })
}

async fn edit_task(field: Field, task: &mut Task, ui: &Ui, screen: &mut Screen) -> Result<Edit> {
    let host = &ui.host;
    let changed = match field {
        Field::Prompt => {
            long_text(
                ui,
                "Task prompt ({{name}} becomes an input)",
                &mut task.prompt,
            )
            .await?
        }
        Field::Skill => {
            let mut items = vec![Item::new("none", "run the prompt as written")];
            let commands = host.all_commands().await.unwrap_or_default();
            items.extend(commands.iter().filter_map(|c| {
                let name = c["name"].as_str()?.strip_prefix("skill:")?;
                Some(Item::new(name, c["description"].as_str().unwrap_or("")))
            }));
            let Some(choice) =
                picker::pick_one(ui, screen, "Run the prompt with a skill", items).await?
            else {
                return Ok(Edit::Unchanged);
            };
            task.skill = (choice != "none").then_some(choice);
            true
        }
        Field::Delay | Field::Every => {
            let (title, slot) = match field {
                Field::Delay => (
                    "Delay before the first run, such as 5m (empty for none)",
                    &mut task.delay,
                ),
                _ => (
                    "Wait between runs, such as 2m (empty for the default)",
                    &mut task.every,
                ),
            };
            let Some(text) = text(ui, title, slot.as_deref().unwrap_or("")).await? else {
                return Ok(Edit::Unchanged);
            };
            let text = text.trim();
            if !text.is_empty() && parse_duration(text).is_none() {
                return Ok(Edit::Invalid(format!(
                    "{text} is not a duration such as 30s, 5m, or 1h30m."
                )));
            }
            *slot = (!text.is_empty()).then(|| text.to_owned());
            true
        }
        Field::Repeat => match number(ui, "Runs (empty for the default)", task.repeat).await? {
            Ok(value) => {
                task.repeat = value;
                true
            }
            Err(edit) => return Ok(edit),
        },
        Field::Until => {
            let current = task.until.clone().unwrap_or_default();
            let Some(text) = text(
                ui,
                "Goal, such as \"CI passed and the PR is merged\" (empty for none)",
                &current,
            )
            .await?
            else {
                return Ok(Edit::Unchanged);
            };
            task.until = (!text.trim().is_empty()).then(|| text.trim().to_owned());
            true
        }
        Field::Confirm => {
            task.confirm = !task.confirm;
            true
        }
        Field::Optional => {
            task.optional = !task.optional;
            true
        }
        _ => false,
    };
    Ok(if changed {
        Edit::Changed
    } else {
        Edit::Unchanged
    })
}

async fn text(ui: &Ui, title: &str, current: &str) -> Result<Option<String>> {
    let options = InputOptions {
        prefill: Some(current.to_owned()),
        ..InputOptions::default()
    };
    ui.host.input_with(title, options).await
}

/// Edits multi-line text in pi's editor dialog; `true` when it changed.
pub async fn long_text(ui: &Ui, title: &str, value: &mut String) -> Result<bool> {
    match ui.host.editor(title, Some(value)).await? {
        Some(text) if text.trim() != value.trim() => {
            *value = text.trim().to_owned();
            Ok(true)
        }
        _ => Ok(false),
    }
}

async fn number(
    ui: &Ui,
    title: &str,
    current: Option<u32>,
) -> Result<std::result::Result<Option<u32>, Edit>> {
    let current = current.map(|n| n.to_string()).unwrap_or_default();
    let Some(text) = text(ui, title, &current).await? else {
        return Ok(Err(Edit::Unchanged));
    };
    let text = text.trim();
    if text.is_empty() {
        return Ok(Ok(None));
    }
    match text.parse::<u32>() {
        Ok(n) if n > 0 => Ok(Ok(Some(n))),
        _ => Ok(Err(Edit::Invalid(format!(
            "{text} is not a whole number above 0."
        )))),
    }
}

fn set_list(slot: &mut Option<Vec<String>>, picked: Option<Picked>) -> bool {
    match picked {
        Some(Picked::Inherit) => *slot = None,
        Some(Picked::List(list)) => *slot = Some(list),
        None => return false,
    }
    true
}

/// The `provider/id` models that pi can use now.
pub async fn models(host: &Host) -> Vec<String> {
    let models = host
        .pi_call("ctx.modelRegistry", "getAvailable", Vec::new())
        .await
        .unwrap_or_default();
    models
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| Some(format!("{}/{}", m["provider"].as_str()?, m["id"].as_str()?)))
        .collect()
}

const INHERIT: &str = "inherit";

/// `Some(None)` for inherit, `None` when dismissed. Typed text that names no listed model is
/// used as `provider/id`.
pub async fn choose_model(
    ui: &Ui,
    screen: &mut Screen,
    title: &str,
    with_ask: bool,
    inherit: &str,
) -> Result<Option<Option<String>>> {
    let mut items = vec![Item::new(INHERIT, inherit)];
    if with_ask {
        items.push(Item::new(ASK, "choose the model when the run starts"));
    }
    items.extend(models(&ui.host).await.into_iter().map(|m| Item::new(m, "")));
    let Some(choice) = picker::pick_one(ui, screen, title, items).await? else {
        return Ok(None);
    };
    Ok(match choice.as_str() {
        INHERIT => Some(None),
        c if c == ASK
            || c.split_once('/')
                .is_some_and(|(p, id)| !p.is_empty() && !id.is_empty()) =>
        {
            Some(Some(choice))
        }
        _ => None,
    })
}

const SUB_INHERIT: &str = "inherit · subagents as configured";
const SUB_NONE: &str = "none · the agent cannot start subagents";
const SUB_RULES: &str = "only these types…";

/// `Some(new rules)` or `None` when dismissed.
async fn subagents(
    ui: &Ui,
    screen: &mut Screen,
    current: Option<&[SubagentRule]>,
) -> Result<Option<Option<Vec<SubagentRule>>>> {
    let Some(choice) = ui
        .host
        .select("Subagents", &[SUB_INHERIT, SUB_NONE, SUB_RULES])
        .await?
    else {
        return Ok(None);
    };
    match choice.as_str() {
        SUB_INHERIT => return Ok(Some(None)),
        SUB_NONE => return Ok(Some(Some(Vec::new()))),
        _ => {}
    }
    let current = current.unwrap_or_default();
    let types: Vec<Item> = crate::app::subagent_types(&ui.host)
        .into_iter()
        .map(|t| Item::new(t, ""))
        .collect();
    let chosen: Vec<String> = current.iter().map(|r| r.agent.clone()).collect();
    let Some(Picked::List(agents)) =
        picker::pick(ui, screen, "Subagent types", types, Some(&chosen), "all").await?
    else {
        return Ok(None);
    };
    let models = models(&ui.host).await;
    let mut rules = Vec::new();
    for agent in agents {
        let old = current
            .iter()
            .find(|r| r.agent == agent)
            .cloned()
            .unwrap_or_default();
        let items = models.iter().map(|m| Item::new(m.clone(), "")).collect();
        let title = format!("Models for {agent} (none checked: any; the first is the default)");
        let models = match picker::pick(ui, screen, &title, items, Some(&old.models), "any").await?
        {
            Some(Picked::List(list)) => list,
            Some(Picked::Inherit) => Vec::new(),
            None => return Ok(None),
        };
        let items = THINKING_LEVELS.iter().map(|t| Item::new(*t, "")).collect();
        let title = format!("Thinking levels for {agent} (none checked: any)");
        let thinking =
            match picker::pick(ui, screen, &title, items, Some(&old.thinking), "any").await? {
                Some(Picked::List(list)) => list,
                Some(Picked::Inherit) => Vec::new(),
                None => return Ok(None),
            };
        rules.push(SubagentRule {
            agent,
            models,
            thinking,
        });
    }
    Ok(Some(Some(rules)))
}

const ENV_HEADER: &str = "# One KEY=value per line. ${NAME} reads the environment of pi, so secrets stay out of the file.";

pub fn parse_env(text: &str) -> std::result::Result<BTreeMap<String, String>, String> {
    let mut env = BTreeMap::new();
    for line in text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("{line:?} is not KEY=value."))?;
        let value = value.trim();
        let unquoted = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value);
        env.insert(key.trim().to_owned(), unquoted.to_owned());
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_lines_parse() {
        let env = parse_env("# comment\nexport A=1\nB = \"two words\"\nC='${SECRET}'\n").unwrap();
        assert_eq!(env["A"], "1");
        assert_eq!(env["B"], "two words");
        assert_eq!(env["C"], "${SECRET}");
        assert!(parse_env("nope").is_err());
    }

    #[test]
    fn step_scope_values_show_what_they_inherit() {
        let workflow: Workflow = serde_json::from_value(json!({
            "name": "w", "agent": { "model": "a/b", "tools": [] },
            "steps": [{ "name": "s", "agent": { "thinking": "low" }, "tasks": [{ "prompt": "p", "repeat": 3 }] }]
        }))
        .unwrap();
        let paint = Paint::default();
        let plain = |field, node| pi_ext_sdk::style::strip(&value(field, node, &workflow, &paint));
        assert_eq!(plain(Field::Model, Node::Step(0)), "inherit · a/b");
        assert_eq!(plain(Field::Tools, Node::Step(0)), "inherit · none");
        assert_eq!(plain(Field::Thinking, Node::Step(0)), "low");
        assert_eq!(plain(Field::Skills, Node::Settings), "inherit · all");
        assert_eq!(plain(Field::Repeat, Node::Task(0, 0)), "3");
        assert_eq!(plain(Field::Every, Node::Task(0, 0)), "none");
    }
}
