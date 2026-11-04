---
name: workflow-create
description: Design a new pi workflow, or change an existing one, from the user's description, and save it with the workflow tool. Use when the user asks for a workflow, pipeline, runbook, or a repeatable multi-agent process (implement, review, ship, deploy, monitor).
---

# Create or edit a workflow

A workflow runs steps in order. One agent runs each step and works through the step's tasks. You save workflows with the `workflow` tool. The user runs them from `/workflows`, or you run them with `workflow` action `run`.

## Procedure

1. Find out the goal, the order of the work, and the points where a person must approve. If the request leaves a material choice open (target branches, deploy commands, which model for which step), ask. Use `ask_user_question` when it is available. Do not ask about details that have a safe default.
2. Read the project to get real commands: package scripts, CI files, migration tools, deploy scripts. Put the real commands into the task prompts.
3. Call `workflow` action `list` to see existing workflows. To edit one, call action `get`, change the JSON, and save it with the same name.
4. Write the workflow with the rules below, then call `workflow` action `save`. If the tool returns problems, fix them and save again.
5. Tell the user the name, the steps, and the inputs, and how to start it (`/workflows`, or ask you to run it).

## Design rules

- One step for each change of role or model: implement, verify, fix, ship, monitor. Put work that needs the same context into the tasks of one step.
- Write each task prompt so an agent that did not see this conversation can do it: the goal, the commands, the branches, and what "done" means.
- Use `{{name}}` placeholders for the facts that change per run, such as `{{change}}` or `{{ticket}}`.
- Pick models by the work. Use a strong model with `high` thinking to implement and to fix. Use a mid model to review. Use a small model with `low` thinking to poll CI or read logs. Use `"model": "ask"` when the user wants to choose at run start.
- Give least privilege. Set `tools`, `mcp`, and `skills` for steps that need few of them. Set `subagents` to `[]` or to a short list of rules when a step must not start other agents.
- Use `until` with `every` and `repeat` for waits on outside systems, such as CI or a deploy. Use `delay`, `repeat`, and `every` for checks on a schedule.
- Set `confirm: true` on tasks that merge to main, deploy to production, run production migrations, or delete data. The run waits for the user's answer, so tell the user that a run waits for them.
- Set `background: true` on workflows that wait for long (CI, deploys, monitors over hours), so they keep going after pi quits.
- Put secrets in `env` as `${NAME}` references to the user's environment. Never write a secret value into the workflow.
- Choose `context`: `handoff` for most workflows, `memory` when later agents need facts that do not fit in a report, `session` when all steps must share one conversation, and `none` for steps that do not need history (monitors).
- Use `mode: "dynamic"` only when the steps depend on what the agents find. Give the planner `instructions` and keep `maxSteps` low.
- Set `maxTurns` or `maxCost` on long or looping steps.

## Example: ship a change

```json
{
  "name": "ship-change",
  "description": "Implement a change, check it against current docs, fix gaps, ship it through dev and staging to main, then watch production.",
  "context": "handoff",
  "background": true,
  "agent": { "model": "anthropic/claude-sonnet-4-5", "thinking": "medium" },
  "steps": [
    { "name": "implement", "agent": { "model": "ask", "thinking": "high" },
      "tasks": [{ "prompt": "Implement this change: {{change}}. Run the tests that cover it." }] },
    { "name": "docs-check", "agent": { "tools": ["read", "bash", "edit", "write", "web_search", "url_context"] },
      "tasks": [{ "prompt": "Check every library and API that the change uses against its latest official docs. Update code that uses old or wrong APIs. Report what you checked." }] },
    { "name": "sanity", "tasks": [{ "prompt": "Review the full diff for bugs, missed cases, and dead code. Fix what you find and run the checks again." }] },
    { "name": "ship", "agent": { "env": { "DATABASE_URL": "${DEV_DATABASE_URL}" } },
      "tasks": [
        { "prompt": "Review the diff.", "skill": "review" },
        { "prompt": "Open a PR to dev with a summary of the change." },
        { "prompt": "Check the PR's CI. When it passes, merge the PR into dev.", "until": "CI passed and the PR is merged into dev", "every": "2m", "repeat": 30 },
        { "prompt": "Run the dev migrations, fix failures, and check the UI with a browser." },
        { "prompt": "Merge dev into staging and run the staging migrations.", "confirm": true },
        { "prompt": "Deploy to main.", "confirm": true }
      ] },
    { "name": "monitor", "context": "none", "agent": { "model": "anthropic/claude-haiku-4-5", "thinking": "low" },
      "tasks": [
        { "prompt": "Check the remote logs and activity for errors after the deploy.", "delay": "5m", "repeat": 3, "every": "5m" },
        { "prompt": "Do a final check of the logs and error rates.", "delay": "1h" }
      ] }
  ]
}
```
