//! Model calls that write workflows: the plan of a dynamic run, and a new workflow from a
//! description.

use pi_ext_sdk::LlmRequest;
use pi_ext_sdk::prelude::*;
use serde::Deserialize;

use crate::model::{Step, Workflow, step_problems};
use crate::runs::RunState;
use crate::store::FORMAT;

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

/// The steps after `index` for a dynamic run.
pub async fn plan(
    host: &Host,
    state: &RunState,
    index: usize,
    available: Value,
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
        "available": available,
    });
    let text = complete(
        host,
        PLAN_RULES,
        &request,
        workflow.planner.model.as_deref(),
    )
    .await?;
    #[derive(Deserialize)]
    struct Plan {
        steps: Vec<Step>,
    }
    let mut plan: Plan = parse(&text)?;
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
    let text = complete(host, GENERATE_RULES, &request, None).await?;
    parse(&text)
}

async fn complete(
    host: &Host,
    rules: &str,
    request: &Value,
    model: Option<&str>,
) -> Result<String, String> {
    let prompt = serde_json::to_string_pretty(request).map_err(|e| e.to_string())?;
    let mut llm = LlmRequest::prompt(prompt).system(format!("{rules}\n\n{FORMAT}"));
    if let Some(model) = model {
        llm = llm.model(model);
    }
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
