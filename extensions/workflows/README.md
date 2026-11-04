# pi-ext-workflows

Created 2026-11-04. Last updated 2026-11-04.

Runs multi-agent workflows. A workflow is a list of steps; one pi agent runs each step and works through the step's tasks in one session. Each step has its own model, thinking level, tools, skills, MCP servers, subagent rules, and environment. Tasks can wait, repeat on a schedule, loop until a goal is met, or ask the user first. Run it with `pi-rust-ext workflows`.

A `fixed` workflow runs its steps as written. A `dynamic` workflow has a planner model write the remaining steps before each step, from the `goal` input, the reports of the finished steps, and the written steps as a starting plan.

## `/workflows`

| Command | Opens |
| --- | --- |
| `/workflows` | The saved workflows and the runs, on two tabs. |
| `/workflows new` | The editor with an empty workflow. |
| `/workflows edit <name>` | The editor for one workflow. |
| `/workflows run <name>` | The run dialog: models marked `ask`, then the `{{inputs}}`. |
| `/workflows runs` | The runs tab. |

Home keys: `enter` edit, `r` run, `n` new, `g` generate (the session model drafts a workflow from your description, and the draft opens in the editor), `c` copy, `d` delete, `tab` switches tabs. On the runs tab: `enter` opens a run, `x` stops it, `s` ends the current wait, and `r` resumes a stopped, failed, or interrupted run.

The editor shows the steps and tasks as a tree on the left and the form of the selected row on the right (stacked below 96 columns). Tree keys: `enter` edits the row, `a` adds a step, `t` adds a task, `J`/`K` move the row, `c` copies it, `d` deletes it, `s` saves, `r` saves and runs, `esc` leaves (it asks when there are unsaved changes). Form keys: `enter` changes the field, `x` resets it to inherit, `esc` returns to the tree. Text fields use pi's input and editor dialogs. Models, skills, tools, MCP servers, and subagent types use a list in the panel: type to filter, `space` checks, `enter` finishes, `ctrl+r` resets to inherit, and typed text that names no item can be used as a value (for example `anthropic/*`).

While a run is active, the widget `workflows` above the editor shows each run's steps and what the current step does or how long it waits. When a run ends, a `workflow-run` message reports each step; it starts a turn when an agent started the run.

## Tool `workflow`

| Param | Use |
| --- | --- |
| `action` | `list`, `get` (`name`), `save` (`workflow`, `scope`), `delete` (`name`), `run` (`name` or `workflow`, `inputs`), `status` (all runs, or one run by `id`), `stop` (`id`), `resume` (`id`) |
| `workflow` | A full definition. `save` validates it and reports each problem with its step and task. |
| `inputs` | Values for the `{{placeholders}}` of the prompts; `goal` for dynamic workflows. |
| `scope` | `global` or `project` for `save`. Default: where the workflow is now, else global. |

`run` returns the run id at once; the run continues in the background. `status` with an id lists each step's status, time, cost, agent runs, turns, model, log, and last report, which the `workflow-optimize` skill uses.

## Skills

The extension writes two skills to `<agentDir>/ext/workflows/skills` and adds the directory through `resources_discover`:

- `workflow-create` designs a workflow from a description and saves it.
- `workflow-optimize` reads a workflow and its runs and makes it faster, cheaper, or more reliable.

Both include the file format reference, `skills/format.md`.

## Files

| Path | Contents |
| --- | --- |
| `<agentDir>/ext/workflows/definitions/<name>.json` | Global workflows |
| `<cwd>/.pi/ext/workflows/definitions/<name>.json` | Project workflows. They load only in a trusted project and hide a global workflow of the same name. |
| `<agentDir>/ext/workflows/runs/<id>/` | `run.json` (state, history, resume), one session file per step (`01-<step>.jsonl`, or `session.jsonl` in `session` context), `memory.md`, `report.schema.json`. Directories older than 30 days are deleted at startup. |

## Step agent

`<pi> --mode json -p --session <file> [--model <m>] [--thinking <t>] [--tools <list> | --no-tools] [--no-mcp] --exclude-tools workflow[,subagent][,mcp__<server>__*] [--no-skills --skill <path>...] --append-system-prompt <report guide and systemPrompt> --output-schema report.schema.json [--max-turns <n>] [--max-cost <usd>] -- <prompt>`, in the cwd of the session that started the run.

- Model and thinking: the step's `agent`, else the workflow `agent`, else the model of the session that started the run. A step that inherits the session model also inherits its thinking level.
- The pi binary: `$PI_BIN`, else the parent process when its file name starts with `pi`, else `pi` on `PATH`.
- Each agent run ends with the JSON report `{ status: done | continue | blocked, summary }`. `blocked`, a pi error, a run limit, or a missed `until` goal stops the workflow unless the task is `optional`. An answer that does not match the schema (exit code 4) counts as `done` with the last text as the summary.
- The first agent run of a step gets a header with the workflow, the step, and, by `context`, the reports of the earlier steps (`handoff`, `memory`) and the shared notes (`memory`). Later runs of the step continue its session, so tasks see the earlier tasks.
- Env: the step's `env`, with `${NAME}` read from the environment of pi, plus `PI_WORKFLOW_RUN`, `PI_WORKFLOW_STEP`, and `PI_TODO_SCOPE=<run id>-<step number>`. `PI_WORKFLOW_SUBAGENTS` carries the step's subagent rules and `PI_WORKFLOW_MEMORY` the path of the shared notes.

In a step agent (`PI_WORKFLOW_RUN` is set) the extension registers only two things. A `tool_call` handler checks each `subagent` call against `PI_WORKFLOW_SUBAGENTS`: it blocks agent types, models, and thinking levels outside the rules, and fills in the first concrete model and first thinking level of the rule when the call gives none. The `workflow_memory` tool (`note`) appends to the shared notes in `memory` context.

## Runs

A task with `confirm` waits for the user's choice: run it, skip it, or stop the workflow. Without a UI the task fails. Waits (`delay`, `every`) show a countdown; `s` in the run view or on the runs tab ends the wait at once. When pi quits, active runs stop as `interrupted`; `resume` continues a run at the task that did not finish, in the same session file.

## Format

See [skills/format.md](skills/format.md) for every field, and [skills/workflow-create.md](skills/workflow-create.md) for a full example: implement, check against current docs, fix, ship through dev and staging with approvals and a CI wait, then watch the logs every 5 minutes three times and once more after an hour.
