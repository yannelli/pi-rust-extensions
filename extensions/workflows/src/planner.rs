//! Model calls that write workflows: the plan of a dynamic run (a `pi -p` run of the runner), and
//! a new workflow from a description (the session's model).

use std::path::Path;

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::{CancellationToken, LlmRequest};
use serde::Deserialize;

use crate::child::{self, Outcome};
use crate::launch::Launch;
use crate::model::{Step, Workflow, step_problems};
use crate::policy::RUN_ENV;
use crate::runs::RunState;
use crate::store::FORMAT;

pub const SCHEMA_FILE: &str = "plan.schema.json";
pub const SCHEMA: &str = r#"{
  "type": "object",
  "properties": { "steps": { "type": "array", "items": { "type": "object" } } },
  "required": ["steps"]
}
"#;

const DEFAULT_MAX_STEPS: usize = 12;

const PLAN_RULES: &str = "You plan the remaining steps of a running pi workflow. Each step is one \
agent session that runs its tasks in order. Read the goal, the instructions, the completed steps \
with their reports, and the current plan. Then return the steps that are still needed, in order. \
Keep planned steps that are still right; change, add, or drop steps when the reports show a need. \
Return an empty list when the goal is reached. Write each task prompt so an agent without this \
context can do it. Answer with one JSON object, {\"steps\": [...]}, and nothing else.";

const GENERATE_RULES: &str = "You design pi workflows. Turn the user's description into one \
workflow. Use the real commands and names from the description. Use {{name}} placeholders for \
facts that change per run. Pick strong models for implementation, small models for polling and \
monitoring, `until` for waits on CI or deploys, and `confirm: true` before merges to main, \
production deploys, and destructive commands. Answer with the workflow JSON object and nothing else.";

/// What the planner may use: models, skills, tools, MCP servers, and subagent types.
pub async fn available(host: &Host, catalog: &crate::launch::Catalog) -> Value {
    let models = host
        .pi_call("ctx.modelRegistry", "getAvailable", Vec::new())
        .await
        .unwrap_or_default();
    let models: Vec<String> = models
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| Some(format!("{}/{}", m["provider"].as_str()?, m["id"].as_str()?)))
        .take(80)
        .collect();
    let tools: Vec<String> = host
        .all_tools()
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .filter(|n| !n.starts_with("mcp__"))
        .collect();
    json!({
        "models": models,
        "skills": catalog.skills.keys().collect::<Vec<_>>(),
        "tools": tools,
        "mcpServers": catalog.mcp_servers,
        "subagents": crate::app::subagent_types(host),
    })
}

/// The steps after `index` for a dynamic run, from a `pi -p` run without tools.
pub async fn plan(
    pi: &Path,
    dir: &Path,
    state: &RunState,
    index: usize,
    cancel: &CancellationToken,
) -> Result<Vec<Step>, String> {
    let workflow = &state.workflow;
    let max = workflow
        .planner
        .max_steps
        .map_or(DEFAULT_MAX_STEPS, |n| n as usize);
    let completed: Vec<Value> = workflow.steps[..index]
        .iter()
        .zip(&state.steps)
        .map(
            |(step, run)| json!({ "name": step.name, "status": run.status, "report": run.summary }),
        )
        .collect();
    let request = json!({
        "workflow": workflow.name,
        "description": workflow.description,
        "instructions": workflow.planner.instructions,
        "inputs": state.inputs,
        "completedSteps": completed,
        "currentPlan": workflow.steps[index..],
        "maxMoreSteps": max.saturating_sub(index),
        "available": state.available,
    });
    let model = workflow
        .planner
        .model
        .clone()
        .or(state.parent_model.clone());
    let mut args: Vec<String> = [
        "--mode",
        "json",
        "-p",
        "--no-session",
        "--no-tools",
        "--no-skills",
        "--no-context-files",
    ]
    .map(String::from)
    .to_vec();
    if let Some(model) = &model {
        args.extend(["--model".into(), model.clone()]);
    }
    args.extend([
        "--system-prompt".into(),
        format!("{PLAN_RULES}\n\n{FORMAT}"),
    ]);
    args.extend([
        "--output-schema".into(),
        dir.join(SCHEMA_FILE).display().to_string(),
    ]);
    let prompt = serde_json::to_string_pretty(&request).map_err(|e| e.to_string())?;
    args.extend(["--".into(), prompt]);
    let launch = Launch {
        program: pi.to_owned(),
        args,
        env: vec![(RUN_ENV.into(), state.id.clone())],
        cwd: state.cwd.clone().into(),
        model,
    };
    let (outcome, fold) = child::run(&launch, cancel, |_| {}).await;
    #[derive(Deserialize)]
    struct Plan {
        steps: Vec<Step>,
    }
    let mut plan: Plan = match (fold.output, outcome) {
        (Some(value), _) => {
            serde_json::from_value(value).map_err(|e| format!("the plan is not valid: {e}"))?
        }
        (None, Outcome::Failed(reason)) => return Err(reason),
        (None, Outcome::Cancelled) => return Err("cancelled".into()),
        (None, Outcome::Report { .. }) => parse(&fold.final_text)?,
    };
    plan.steps.truncate(max.saturating_sub(index));
    let problems: Vec<String> = plan
        .steps
        .iter()
        .enumerate()
        .flat_map(|(i, step)| step_problems(step, &format!("planned step {}", i + 1)))
        .collect();
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    Ok(plan.steps)
}

/// A draft workflow from the user's description; the editor shows its problems.
pub async fn generate(
    host: &Host,
    description: &str,
    available: Value,
) -> Result<Workflow, String> {
    let request = json!({ "description": description, "available": available });
    let text = complete(host, GENERATE_RULES, &request).await?;
    parse(&text)
}

async fn complete(host: &Host, rules: &str, request: &Value) -> Result<String, String> {
    let prompt = serde_json::to_string_pretty(request).map_err(|e| e.to_string())?;
    let llm = LlmRequest::prompt(prompt).system(format!("{rules}\n\n{FORMAT}"));
    let response = host
        .complete(llm)
        .await
        .map_err(|e| e.message().to_owned())?;
    Ok(response.text)
}

/// The JSON object in a model answer, with or without a code fence.
fn parse<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    let start = text.find('{').ok_or("the model answered without JSON")?;
    let end = text.rfind('}').ok_or("the model answered without JSON")?;
    serde_json::from_str(&text[start..=end])
        .map_err(|e| format!("the model's JSON is not a valid workflow: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fenced_json() {
        let text = "Here:\n```json\n{\"name\": \"x\", \"steps\": []}\n```";
        let workflow: Workflow = parse(text).unwrap();
        assert_eq!(workflow.name, "x");
        assert!(parse::<Workflow>("no json").is_err());
        assert!(
            parse::<Workflow>("{\"nme\": 1}")
                .unwrap_err()
                .contains("nme")
        );
    }
}
