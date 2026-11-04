//! The extension inside a step agent: it enforces the step's subagent rules on `subagent` calls
//! and adds the `workflow_memory` tool in memory context.

use pi_ext_sdk::prelude::*;
use serde::Deserialize;

use crate::model::{SubagentRule, glob_match};

pub const RUN_ENV: &str = "PI_WORKFLOW_RUN";
pub const STEP_ENV: &str = "PI_WORKFLOW_STEP";
pub const SUBAGENTS_ENV: &str = "PI_WORKFLOW_SUBAGENTS";
pub const MEMORY_ENV: &str = "PI_WORKFLOW_MEMORY";
pub const MEMORY_TOOL: &str = "workflow_memory";

/// Checks a `subagent` call against the rules. `Ok(Some(input))` fills in default models and
/// thinking levels; `Err` names what the step allows.
pub fn enforce(
    input: &Value,
    rules: &[SubagentRule],
) -> std::result::Result<Option<Value>, String> {
    if input["action"].as_str().is_some_and(|a| a != "run") {
        return Ok(None);
    }
    let mut input = input.clone();
    let default_agent = input["agent"].as_str().unwrap_or("explore").to_owned();
    let mut changed = false;
    if let Some(tasks) = input.get_mut("tasks").and_then(Value::as_array_mut) {
        for task in tasks {
            if task["agent"].is_null() {
                task["agent"] = json!(default_agent);
            }
            changed |= check_call(task, rules)?;
        }
    } else {
        if input["agent"].is_null() {
            input["agent"] = json!(default_agent);
        }
        changed |= check_call(&mut input, rules)?;
    }
    Ok(changed.then_some(input))
}

fn check_call(call: &mut Value, rules: &[SubagentRule]) -> std::result::Result<bool, String> {
    let agent = call["agent"].as_str().unwrap_or("explore").to_owned();
    let Some(rule) = rules.iter().find(|r| r.agent == agent) else {
        let allowed: Vec<&str> = rules.iter().map(|r| r.agent.as_str()).collect();
        return Err(format!(
            "This workflow step allows only these subagents: {}.",
            allowed.join(", ")
        ));
    };
    let mut changed = false;
    match call["model"].as_str() {
        Some(model)
            if !rule.models.is_empty() && !rule.models.iter().any(|p| glob_match(p, model)) =>
        {
            return Err(format!(
                "Subagent {agent} may use only these models: {}.",
                rule.models.join(", ")
            ));
        }
        Some(_) => {}
        None => {
            if let Some(model) = rule.models.iter().find(|m| !m.contains('*')) {
                call["model"] = json!(model);
                changed = true;
            }
        }
    }
    match call["thinking"].as_str() {
        Some(level) if !rule.thinking.is_empty() && !rule.thinking.iter().any(|t| t == level) => {
            return Err(format!(
                "Subagent {agent} may use only these thinking levels: {}.",
                rule.thinking.join(", ")
            ));
        }
        Some(_) => {}
        None => {
            if let Some(level) = rule.thinking.first() {
                call["thinking"] = json!(level);
                changed = true;
            }
        }
    }
    Ok(changed)
}

#[derive(Deserialize)]
struct NoteArgs {
    note: String,
}

/// The extension for a step agent, from the variables that the workflow run sets.
pub fn child_extension() -> Extension {
    let mut ext = Extension::new("workflows");
    if let Some(rules) = std::env::var(SUBAGENTS_ENV)
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<SubagentRule>>(&text).ok())
    {
        ext = ext.on("tool_call", move |ev: EventCtx| {
            let rules = rules.clone();
            async move {
                if ev.get("toolName").and_then(Value::as_str) != Some("subagent") {
                    return Ok(EventReply::none());
                }
                let input = ev.get("input").cloned().unwrap_or(Value::Null);
                Ok(match enforce(&input, &rules) {
                    Ok(Some(input)) => EventReply::patch(json!({ "input": input })),
                    Ok(None) => EventReply::none(),
                    Err(reason) => EventReply::block(reason),
                })
            }
        });
    }
    if let Ok(path) = std::env::var(MEMORY_ENV) {
        let spec = ToolSpec::new(
            MEMORY_TOOL,
            "Add a note to the workflow's shared memory. Later steps of this workflow read it. Write \
facts they need: decisions, commands that worked, gotchas, where things are. Do not write secrets.",
        )
        .label("Workflow memory")
        .parameters(json!({
            "type": "object",
            "properties": { "note": { "type": "string", "description": "One self-contained note in Markdown" } },
            "required": ["note"]
        }));
        ext = ext.tool(spec, move |args: NoteArgs, _ctx: ToolCtx| {
            let path = path.clone();
            async move {
                let step = std::env::var(STEP_ENV).unwrap_or_default();
                let mut text = std::fs::read_to_string(&path).unwrap_or_default();
                text.push_str(&format!("\n- ({step}) {}\n", args.note.trim()));
                std::fs::write(&path, text)?;
                Ok("Saved to the workflow memory.")
            }
        });
    }
    ext
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Vec<SubagentRule> {
        vec![
            SubagentRule {
                agent: "explore".into(),
                models: vec!["anthropic/claude-haiku-4-5".into(), "anthropic/*".into()],
                thinking: vec!["low".into(), "medium".into()],
            },
            SubagentRule {
                agent: "build".into(),
                ..SubagentRule::default()
            },
        ]
    }

    #[test]
    fn fills_defaults_and_blocks_outside_the_rules() {
        let patched = enforce(&json!({"task": "find x"}), &rules())
            .unwrap()
            .unwrap();
        assert_eq!(patched["agent"], "explore");
        assert_eq!(patched["model"], "anthropic/claude-haiku-4-5");
        assert_eq!(patched["thinking"], "low");
        assert_eq!(
            enforce(&json!({"agent": "build", "task": "x"}), &rules()).unwrap(),
            None
        );
        let err = enforce(&json!({"agent": "advisor", "task": "x"}), &rules()).unwrap_err();
        assert!(err.contains("explore, build"), "{err}");
        let err = enforce(&json!({"task": "x", "model": "openai/gpt-5"}), &rules()).unwrap_err();
        assert!(err.contains("anthropic/*"), "{err}");
        let err = enforce(&json!({"task": "x", "thinking": "xhigh"}), &rules()).unwrap_err();
        assert!(err.contains("low, medium"), "{err}");
        assert_eq!(
            enforce(&json!({"action": "status"}), &rules()).unwrap(),
            None
        );
    }

    #[test]
    fn checks_every_parallel_task() {
        let input = json!({"agent": "build", "tasks": [{"task": "a"}, {"task": "b", "agent": "explore", "model": "anthropic/x"}]});
        let patched = enforce(&input, &rules()).unwrap().unwrap();
        assert_eq!(patched["tasks"][0]["agent"], "build");
        assert_eq!(patched["tasks"][1]["model"], "anthropic/x");
        assert_eq!(patched["tasks"][1]["thinking"], "low");
        let input = json!({"tasks": [{"task": "a", "agent": "advisor"}]});
        assert!(enforce(&input, &rules()).is_err());
    }
}
