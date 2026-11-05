# pi-ext-workflows

Created 2026-11-04. Last updated 2026-11-04.

Runs multi-agent workflows. A workflow is a list of steps; one pi agent runs each step and works through the step's tasks in one session. Each step has its own model, thinking level, tools, skills, MCP servers, subagent rules, and environment. Tasks can wait, repeat on a schedule, loop until a goal is met, or wait for the user's approval. Run it with `pi-rust-ext workflows`.

Each run has a runner process of its own. A background run keeps going after pi quits; a foreground run stops when the pi that started it quits, and can resume later.

A `fixed` workflow runs its steps as written. A `dynamic` workflow has a planner model write the remaining steps before each step, from the `goal` input, the reports of the finished steps, and the written steps as a starting plan.

## `/workflows`

| Command | Opens |
| --- | --- |
| `/workflows` | The saved workflows and the runs, on two tabs. |
| `/workflows new` | The editor with an empty workflow. |
| `/workflows edit <name>` | The editor for one workflow. |
| `/workflows run <name>` | The run dialog: models marked `ask`, then the `{{inputs}}`. |
| `/workflows runs` | The runs tab. |
| `/workflows approve [<run id>] [run \| skip \| stop]` | Answers a waiting approval; without an id, the one run that waits. It works without a UI: `pi -p "/workflows approve wf-abc"`. |

Home keys: `enter` edit, `r` run, `n` new, `g` generate (the session model drafts a workflow from your description, and the draft opens in the editor), `c` copy, `d` delete, `tab` switches tabs. On the runs tab and in the run view: `enter` opens a run, `x` stops it, `s` ends the current wait, `b` switches it between background and foreground, `a` approves the task that waits, `n` skips that task, `m` sends a message to the agent that works now, and `r` resumes a stopped, failed, or interrupted run. The key line shows the keys that apply to the selected run.

The run view shows the steps and, for the selected step, its log, its report, and the output of its agent: the task prompt, the agent's messages, its tool calls, failed tool results, and the text it writes now. It follows the run's current step until you pick a step with `↑↓`, and stays at the newest output until you scroll with `pgup`. `end` goes back to the newest output. A message sent with `m` reaches the agent as steering after its current tool call, and appears in the output with `›`.

The editor shows the steps and tasks as a tree on the left and the form of the selected row on the right (stacked below 96 columns). Tree keys: `enter` edits the row, `a` adds a step, `t` adds a task, `J`/`K` move the row, `c` copies it, `d` deletes it, `s` saves, `r` saves and runs, `esc` leaves (it asks when there are unsaved changes). Form keys: `enter` changes the field, `x` resets it to inherit, `esc` returns to the tree. Text fields use pi's input and editor dialogs. Models, skills, tools, MCP servers, and subagent types use a list in the panel: type to filter, `space` checks, `enter` finishes, `ctrl+r` resets to inherit, and typed text that names no item can be used as a value (for example `anthropic/*`).

While a run is active, the widget `workflows` above the editor shows each run's steps, `bg` for a background run, and what the current step does, how long it waits, or which task waits for approval. When pi starts while runs are active, a notification names them and points to `/workflows runs`. When a run ends, a `workflow-run` message reports each step; it starts a turn when an agent of this pi started the run. A run that ends while no pi watches it is reported by the next pi session with a UI.

## Tool `workflow`

| Param | Use |
| --- | --- |
| `action` | `list`, `get` (`name`), `save` (`workflow`, `scope`), `delete` (`name`), `run` (`name` or `workflow`, `inputs`, `background`), `status` (all runs, or one run by `id`), `stop` (`id`), `resume` (`id`), `background` (`id`), `foreground` (`id`) |
| `workflow` | A full definition. `save` validates it and reports each problem with its step and task. |
| `inputs` | Values for the `{{placeholders}}` of the prompts; `goal` for dynamic workflows. |
| `scope` | `global` or `project` for `save`. Default: where the workflow is now, else global. |
| `background` | For `run`: keep the run going after pi quits. Default: the workflow's `background`, and always `true` when pi has no UI (print mode). |

`run` returns the run id at once; the agent goes on while the run works. The tool cannot answer approvals; the user answers them. `status` with an id lists each step's status, time, cost, agent runs, turns, model, log, and last report, which the `workflow-optimize` skill uses.

## Skills

The extension writes two skills to `<agentDir>/ext/workflows/skills` and adds the directory through `resources_discover`:

- `workflow-create` designs a workflow from a description and saves it.
- `workflow-optimize` reads a workflow and its runs and makes it faster, cheaper, or more reliable.

Both include the file format reference, `skills/format.md`.

## Files

| Path | Contents |
| --- | --- |
| `<agentDir>/ext/workflows/definitions/<name>.json` | Global workflows |
| `<cwd>/.pi/ext/workflows/definitions/<name>.json` | Project workflows. They load only in a trusted project and hide a global workflow of the same name. In a git worktree without this folder, the main checkout's folder applies. |
| `<agentDir>/ext/workflows/runs/<id>/` | `run.json` (state, history, resume), one session file per step (`01-<step>.jsonl`, or `session.jsonl` in `session` context), `memory.md`, `report.schema.json`, `plan.schema.json`, `commands/` (commands to the runner), `runner.log` (the runner's stderr), and `reported-<endedMs>` (the claim of the pi that reported the run's end). Directories older than 30 days are deleted at startup. |
| `<agentDir>/ext/workflows/sessions/<pid>` | The liveness file of each running pi, refreshed every 2 seconds. |

## Step agent

`<pi> --mode json -p --session <file> [--model <m>] [--thinking <t>] [--tools <list> | --no-tools] [--no-mcp] --exclude-tools workflow[,subagent][,mcp__<server>__*] [--no-skills --skill <path>...] --append-system-prompt <report guide and systemPrompt> --output-schema report.schema.json [--max-turns <n>] [--max-cost <usd>] -- <prompt>`, in the cwd of the session that started the run.

- Model and thinking: the step's `agent`, else the workflow `agent`, else the model of the session that started the run. A step that inherits the session model also inherits its thinking level.
- The pi binary: `$PI_BIN`, else the pi that starts the run (the parent process of the extension, when its file name starts with `pi`), else `pi` on `PATH`. The run keeps the path in `run.json`.
- The planner of a dynamic workflow is a `pi -p --no-session --no-tools --no-skills --no-context-files` run with the planner rules as the system prompt and `plan.schema.json` as the output schema.
- Each agent run ends with the JSON report `{ status: done | continue | blocked, summary }`. `blocked`, a pi error, a run limit, or a missed `until` goal stops the workflow unless the task is `optional`. An answer that does not match the schema (exit code 4) counts as `done` with the last text as the summary.
- The first agent run of a step gets a header with the workflow, the step, and, by `context`, the reports of the earlier steps (`handoff`, `memory`) and the shared notes (`memory`). Later runs of the step continue its session, so tasks see the earlier tasks.
- Env: the step's `env`, with `${NAME}` read from the environment of pi, plus `PI_WORKFLOW_RUN`, `PI_WORKFLOW_STEP`, `PI_WORKFLOW_INBOX`, and `PI_TODO_SCOPE=<run id>-<step number>`. `PI_WORKFLOW_SUBAGENTS` carries the step's subagent rules and `PI_WORKFLOW_MEMORY` the path of the shared notes.

In a step agent or the planner (`PI_WORKFLOW_RUN` is set) the extension registers only three things. While the agent works, it passes each message in `PI_WORKFLOW_INBOX` to the agent as steering. A `tool_call` handler checks each `subagent` call against `PI_WORKFLOW_SUBAGENTS`: it blocks agent types, models, and thinking levels outside the rules, and fills in the first concrete model and first thinking level of the rule when the call gives none. The `workflow_memory` tool (`note`) appends to the shared notes in `memory` context.

## Runs

pi starts `pi-rust-ext workflows --runner <run dir>` for each run, in its own process group, with no pipes to pi, so it outlives pi. The runner owns `run.json`: it writes the state and a heartbeat each second. pi sessions read it, and send commands as JSON files in `commands/` (`stop`, `interrupt`, `skip`, `approve`, `mode`). The runner reads them every 250 ms. It ignores SIGHUP.

- Foreground and background: `run.json` has `background` and `owner`, the process id of the pi that started or resumed the run. A foreground runner stops the run as `interrupted` when its owner quits (pi sends `interrupt`) or when the owner's liveness file is more than 10 seconds old. `/reload` and session switches keep the pi process, so its runs go on. `b`, or the tool actions `background` and `foreground`, switch a running run; switching to the foreground makes the current pi the owner.
- Approvals: a task with `confirm` sets `approval` in `run.json` and waits for an `approve` command, without a time limit. Each pi session with a UI asks once per approval; the first answer wins, and the other sessions close their dialog. `/workflows approve` answers without a UI. When the run is interrupted during the wait, `resume` asks again.
- Waits (`delay`, `every`) show a countdown; `s` ends the wait at once. When an interrupted run resumes, its first wait uses the deadline it had, so a wait does not start over.
- Output: the runner appends the output of each step's agent to `output/<step number>.jsonl`, one `{kind, text}` entry per line (`prompt`, `user`, `assistant`, `tool`, `error`), and keeps the end of the message the agent writes now in the step's `writing`. A message for the agent is a JSON string file in `inbox/`; messages that the agent did not read before it finished go to the step log.
- Reports: the owner reports the end of its runs. A pi session with a UI reports the end of a run whose owner is gone. The pi that creates `reported-<endedMs>` first reports it, so each end is reported once.
- A pi session marks an active run whose heartbeat is more than 30 seconds old as `interrupted`, for example after a restart of the computer. `resume` continues a run at the task that did not finish, in the same session file.

## Format

See [skills/format.md](skills/format.md) for every field, and [skills/workflow-create.md](skills/workflow-create.md) for a full example: implement, check against current docs, fix, ship through dev and staging with approvals and a CI wait, then watch the logs every 5 minutes three times and once more after an hour.
