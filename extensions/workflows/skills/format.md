## Workflow file format

A workflow is one JSON object. The `workflow` tool saves it as `<name>.json` in `~/.pi/agent/ext/workflows/definitions` (global) or `.pi/ext/workflows/definitions` (project). Unknown fields are errors.

```json
{
  "name": "ship-change",
  "description": "Implement a change, check it against current docs, fix gaps, ship it, and watch production.",
  "mode": "fixed",
  "context": "handoff",
  "agent": { "model": "anthropic/claude-sonnet-4-5", "thinking": "medium" },
  "steps": [
    {
      "name": "implement",
      "agent": { "model": "ask", "thinking": "high" },
      "tasks": [{ "prompt": "Implement this change: {{change}}" }]
    },
    {
      "name": "monitor",
      "context": "none",
      "agent": { "model": "anthropic/claude-haiku-4-5", "thinking": "low", "tools": ["bash", "read"] },
      "tasks": [
        { "prompt": "Check the production logs for errors since the deploy.", "delay": "5m", "repeat": 3, "every": "5m" },
        { "prompt": "Do a final check of the logs and error rates.", "delay": "1h" }
      ]
    }
  ]
}
```

### Workflow fields

| Field | Meaning |
| --- | --- |
| `name` | Required. Lowercase letters, digits, and dashes; it is the file name. |
| `description` | What the workflow does. Every step agent sees it. |
| `mode` | `fixed` (default) runs the steps as written. `dynamic` has a planner model write the remaining steps before each step, from the `goal` input, the reports so far, and the written steps as a starting plan. |
| `context` | What a step agent learns about earlier steps. `handoff` (default): the reports of the earlier steps. `memory`: the reports, plus a shared notes file that each agent reads and adds to with the `workflow_memory` tool. `session`: all steps continue one conversation, which pi compacts when it grows. `none`: only its own tasks. |
| `agent` | Default agent settings for all steps (see below). |
| `planner` | Dynamic mode only: `instructions` (rules for the plan), `model` (`provider/id`, default: the session model), `maxSteps` (default 12). |
| `steps` | The steps, in run order. A dynamic workflow may start with none. |

### Step fields

| Field | Meaning |
| --- | --- |
| `name` | Required, unique in the workflow. |
| `context` | Overrides the workflow `context` for this step. |
| `agent` | Agent settings; unset fields come from the workflow `agent`, then from the pi session that starts the run. |
| `tasks` | Required, at least one. One agent session runs the tasks in order, so later tasks see the earlier ones. |

### Agent settings (`agent`)

| Field | Meaning |
| --- | --- |
| `model` | `provider/id`, or `ask` to choose the model when the run starts. |
| `thinking` | `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, or `max`. |
| `tools` | Tool names or `*` patterns the agent may use. Unset: pi's default tools. `[]`: no tools. |
| `skills` | Skill names the agent loads. Unset: all skills. `[]`: none. Skills that tasks run always load. |
| `mcp` | MCP server names the agent may use. Unset: all servers. `[]`: no MCP. |
| `subagents` | Unset: subagents as configured. `[]`: no `subagent` tool. A list allows only these agent types, for example `[{ "agent": "explore", "models": ["anthropic/claude-haiku-4-5", "anthropic/*"], "thinking": ["low"] }]`. The first model without `*` and the first thinking level are the defaults. |
| `env` | Environment variables, for example `{ "DATABASE_URL": "${STAGING_DATABASE_URL}" }`. `${NAME}` reads the environment of pi, so secrets stay out of the file. |
| `systemPrompt` | Text added to the agent's system prompt. |
| `maxTurns` | Stop the agent after this many turns of one task run. |
| `maxCost` | Stop the agent when its session cost reaches this many US dollars. |

### Task fields

| Field | Meaning |
| --- | --- |
| `prompt` | What to do. `{{name}}` placeholders become inputs that the user gives when the run starts. |
| `skill` | Run the prompt as `/skill:<skill> <prompt>`. |
| `delay` | Wait before the first run: `30s`, `5m`, `1h30m`, `2d`. |
| `repeat` | Number of runs (default 1). With `until`: the most runs (default 10). |
| `every` | Wait between runs (default: none; with `until`: `1m`). |
| `until` | A goal. The task runs again until the agent reports it met, for example `"CI passed and the PR is merged into dev"`. |
| `confirm` | `true`: the user must approve the task before it runs. Use it before deploys, merges to main, and destructive commands. |
| `optional` | `true`: a failure skips the rest of the task, and the workflow continues. |

Each agent run ends with a JSON report: `status` (`done`, `continue`, or `blocked`) and `summary`. `blocked`, an error, or a missed `until` goal stops the workflow unless the task is `optional`. A stopped or interrupted run can resume at the task that did not finish.
