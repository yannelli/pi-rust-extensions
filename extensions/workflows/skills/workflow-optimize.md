---
name: workflow-optimize
description: Review a pi workflow and its run history, then make it faster, cheaper, or more reliable. Use when the user asks to optimize, tune, debug, or speed up a workflow, or when a workflow run failed.
---

# Optimize a workflow

## Procedure

1. Call `workflow` action `get` with the name to read the definition.
2. Call `workflow` action `status` to list the runs, then `status` with each recent `id` of this workflow. Each step shows its status, time, cost, agent runs, model, log, and last report.
3. Find the problems. Rank them by effect: failures first, then cost, then time.
4. Change the definition. Keep what the workflow does; change how it does it.
5. Show the user each change and its reason in a short list. Save with `workflow` action `save` and the same name when the user agrees, or at once when the user asked you to apply the changes.

## What to look for

| Sign in the runs | Change |
| --- | --- |
| A step fails or reports `blocked` | Make the prompt name the commands and the meaning of "done". Give the step the tools, MCP servers, skills, or `env` it lacked. Use a stronger model or more thinking. |
| A step costs much, and its report shows simple work | Use a smaller model or less thinking. Set `tools` to the few it needs. |
| An `until` task uses many runs | Make `every` longer. Make the goal concrete and checkable, for example "the `gh pr checks` output shows all checks passed". |
| An `until` task stops at `repeat` with the goal not met | Raise `repeat`, or make `every` longer to cover the real wait. |
| Two steps use the same model and redo each other's reading | Merge them into one step with two tasks. |
| One step does unrelated jobs | Split it, so each step gets the right model and tools. |
| Agents repeat work that an earlier step did | Use `context: "memory"`, or ask the earlier step to report the facts in its summary. |
| A monitor step reads the full history | Set its `context` to `none`. |
| Runs stop with a turn or cost limit | Raise `maxTurns` or `maxCost`, or split the task. |
| A task that may fail stops the whole workflow, but later steps do not need it | Set `optional: true`. |
| A risky task (merge to main, deploy, production migration) runs without a check | Set `confirm: true`. |

Do not remove `confirm` gates or widen tool access without the user's approval. Do not put secret values into `env`; use `${NAME}` references.
