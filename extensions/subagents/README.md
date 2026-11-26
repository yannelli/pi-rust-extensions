# pi-ext-subagents

Created 2026-10-30. Last updated 2026-11-25.

Runs tasks in child pi processes. Three built-in agents: `explore` (read-only search), `build` (full tools), and `advisor` (read-only review with a recommendation). You can change them and add your own. Run it with `pi-rust-ext subagents`. Simplified from `pi-subagents`.

## Tool `subagent`

| Param | Use |
| --- | --- |
| `action` | `list` (agents), `status` (all runs, or one run by `id`), `stop` (`id`). Omit it to run `task` or `tasks`. |
| `agent` | Agent name. Default `explore`. |
| `task` | The full task. The child does not see the parent conversation. |
| `tasks` | `[{ agent?, task, model?, thinking?, cwd? }]`: tasks that run in parallel (`maxParallel` at a time). |
| `async` | `true` runs in the background. The tool returns the run ids at once. In a session with an id, a runner process outside pi runs the children, so they go on when pi stops or restarts (see [Background runs](#background-runs)). A `subagent-notify` message comes once when the runs finish (`deliverAs: followUp`, `triggerTurn: true`). After delivery the extension emits `agent:wake` on `pi.events`, which ends a running `sleep` of the `timers` extension. |
| `model`, `thinking` | Override for this call. |
| `cwd` | Child working directory, relative to the session cwd. A path that resolves outside the session cwd is refused. |

Foreground runs send `tool.update` with each child tool call and the tail of the child's text. Background runs show in the widget `subagents` below the editor. The result `details` is `{ mode, runId?, results, runs }`:

- `mode`: `single` for a `task` call, `parallel` for a `tasks` call. `runId`: the run id of a `single` call.
- `results`: one row `{ index, agent, exitCode?, success, sessionFile }` per finished run. `success` is `true` for a completed run only. `exitCode` is absent when the child did not exit by itself.
- `runs`: `[{ id, agent, task, status, background, model, turns, tools, usage, output, exitCode, durationMs, sessionFile }]`.
- An `async` call returns `results: []`, `asyncId` (the first run id), and `asyncDir` (the first run's directory).
- A foreground `tasks` call also returns `status: "batch"` and `children: [{ id, name, agent, task, status, sessionFile }]` (one per run, `name` is the agent name) when every run wrote its session file. An `async` call never returns them.

These are the fields of the `pi-subagents` result, so Paseo links each `subagent` call with `agent` and `task` and no `action` to its child and shows the child session. `status` and `children` are the batch fields of the `edxeth/pi-subagents` result, so Paseo also shows each child of a foreground `tasks` call. Paseo reads the arguments as the model sent them, so the schema does not list `action: "run"`. A call with `action: "run"` still runs: the extension removes the action in `prepareArguments`, before pi validates the call. An abort of the tool call kills the child's process group; on Windows, `taskkill /T /F` ends the child's process tree. On shutdown the extension cancels the runs of its own process and waits up to 1.4 seconds, so the group gets SIGKILL 1 second after SIGTERM. Detached background runs go on.

## Child process

`<pi> --mode json -p --session <run dir>/session.jsonl [--model <m>] [--thinking <t>] (--tools <list> | --no-tools | --exclude-tools subagent[,<disallowedTools>]) (--append-system-prompt | --system-prompt) <prompt> [--max-turns <n>] [extraArgs] -- <task>`, in the session cwd.

- Model and thinking level: the call override, else the agent's `model`, else the parent session's current model. When neither the call nor the agent sets a model, the child also gets the parent's thinking level, unless the agent or call sets one.
- The pi binary: config `piBin`, else `$PI_BIN`, else the parent process when its file name starts with `pi`, else `pi` on `PATH`.
- Env: `PI_TODO_SCOPE=<run id>`, `PI_SUBAGENT_RUN_ID=<run id>`, `PI_SUBAGENT_AGENT=<name>`, and with `messaging` on, `PI_SUBAGENT_PARENT=<parent session id>`. An agent with a `tools` list also gets `agent_message`.
- Todo scopes: each child `todo` tool result is sent on the `pi.events` channel `todo:scope` as `{ scope, label, status, state }`, with `status` `running`, `completed`, `failed`, or `cancelled` and `state` the child's todo `details` (or `null`). The `todo` extension shows these scopes.
- Lifecycle: `subagents:start` `{ agentId, agentType }` when a run starts and `subagents:stop` `{ agentId, agentType, status, lastAssistantMessage }` when it ends, on `pi.events`. The `hooks` extension runs SubagentStart and SubagentStop hooks from them. A process that adopts a background run after a restart sends no second `subagents:start`.

## Tool `agent_message`

Sends a message between the parent session and its subagent runs. Params: `to` (`parent` from a subagent, or the id of a running run) and `message`. A subagent can also message a running sibling by its run id.

Each agent has a mailbox directory `<agentDir>/ext/subagents/mailbox/<address>/`: the session id for the parent, the run id for a run. The tool writes the message there as a JSON file with a temp-file rename. Each agent reads its mailbox every 500 ms and sends each message as a custom message `agent-message` (`deliverAs: steer`, `triggerTurn: true`), so a working agent sees it at its next step and an idle agent starts a turn. The file is removed only after pi accepts the message, so messages survive a failed send or a `/reload`. After delivery the extension emits `agent:wake`. A run removes its empty mailbox when it exits.

The config key `messaging: false` removes the tool and stops mailbox delivery.

## Run directories

Each run has the directory `<agentDir>/ext/subagents/runs/<run id>/`. The extension makes it when the run starts, and the directory must not exist yet, so two pi processes never use the same run id. The child writes its pi session to `session.jsonl` there. When a call starts runs, the extension deletes run directories last modified more than 14 days ago, except the runs of a background call that runs or waits for its notify.

An `async` call writes `<asyncDir>/status.json` when it starts and after each run ends (the runner writes it for a detached call): `{ state, steps: [{ agent, status, sessionFile }] }`. `state` is `running` until all runs end, then `complete` when all completed, `stopped` when one was cancelled, else `failed`. A step `status` is `running`, `completed`, `failed`, or `stopped`.

The `subagent-notify` message starts with `Background task completed|failed|stopped: **<agent>** (<run id>)` for one run. For several runs it starts with `Background tasks completed (N):`, or `Background tasks failed (N):` / `Background tasks stopped (N):` with the same rules as `state`. Then for each run come `Retention-managed async directory: <run dir>` and `Session file: <file>`, then the run summary and results. Its `details` are those of the tool result.

## Background runs

Paseo stops pi when a runtime is idle for 60 minutes and when its daemon restarts or reloads: stdin EOF, then SIGTERM to pi and each descendant, then SIGKILL after 2 seconds (`taskkill /T /F` on Windows). It then resumes the session with `--session <file>`, which keeps the session id. So in a session with an id, an `async` call runs detached from pi. Without a session id, or when the runner does not start, its runs stay in the extension process and end with it, as foreground runs do. When pi does not accept the notify of such a call (for example during `/reload`), the extension retries 6 times with backoff.

1. Launch. The extension writes `<asyncDir>/call.json`: the session id and file, the mode, and each run's id, agent, task, and child command with the pi binary resolved in pi. It then runs `pi-rust-ext subagents --detach <asyncDir>`, which starts `pi-rust-ext subagents --runner <asyncDir>` and exits at once. The runner's parent is then not pi, so a kill of pi's process tree does not reach the runner or its children. On Unix the runner gets a process group of its own. On Windows it starts with `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB`, and again without the breakaway when pi's job does not allow it.
2. Runner. The runner ignores SIGHUP and starts the children in call order, `maxParallel` at a time. Each child writes its events to `<run dir>/events.jsonl` and its stderr to `<run dir>/stderr.log`, files that do not depend on the runner. The runner folds the events into `<asyncDir>/run.json`, which it also rewrites every 2 seconds as a heartbeat. It keeps `status.json` current and writes `result.json` (through a rename) when all runs end. Its own errors go to `<asyncDir>/runner.log`.
3. Adoption. Every 500 ms from the session start, each extension process reads the calls of its own session id and adds their runs to its run table. The widget, `status`, `stop`, `/agents runs`, and `agent_message` then work for them, and the extension sends `todo:scope`, `subagents:stop`, and the active work block as the runs change. A process does not adopt the calls of another session. A cancel from `stop` or the runs panel writes `<asyncDir>/commands/stop-<run id>`, and the runner then stops that child.
4. One notify. When `result.json` is there, the process that holds `<asyncDir>/notify.claim` sends the `subagent-notify` message. A claim that its holder did not refresh for 10 seconds is free. pi only queues a message that it accepts during a turn, so the extension writes `<asyncDir>/notify.done` only when the message is in the session entries. A process sends nothing for a call with `notify.done`, or for a call whose message (by `details.runs[].id`) is already in the session. So each call gets one message across restarts.
5. A stopped runner. When `run.json` has no new heartbeat for 30 seconds and there is no `result.json` (for example after `kill -9` of the runner), the extension marks the running runs failed with "the background runner stopped before the run finished", writes `status.json`, and sends one notify. The children of that runner go on until they end.

## Agent files

Built-in defaults, then `<agentDir>/agents/*.md`, `<agentDir>/ext/subagents/agents/*.md`, `<cwd>/.pi/agents/*.md`, and `<cwd>/.pi/ext/subagents/agents/*.md`. `<agentDir>/agents` and `.pi/agents` are the folders other pi subagent extensions use (`pi-subagents` among them), so agents written for those load here too. A later file overrides only the fields it sets. A file with a new name adds an agent. Project agents load only when the project is trusted. The format is the Claude Code agent format (YAML frontmatter and a prompt body), so `claude-marketplace` writes plugin agents here. In a git worktree without a project agents folder, the main checkout's copy of that folder applies.

```markdown
---
name: reviewer
description: Reviews a diff and lists bugs.
tools: Read, Grep, Glob   # Claude names map to read, grep, find; `all` gives every tool
disallowedTools: Bash     # removed from the child's tools
model: inherit            # or provider/id; inherit, sonnet, opus, haiku use the session model
thinking: high            # pi only
maxTurns: 20              # pi only
systemPromptMode: append  # append (default) or replace
exposure: deferred        # pi-rust only: direct or deferred, see Deferred agents
---
You review code...
```

## Command `/agents`

`/agents` opens one panel. The header has two tabs, Agents and Runs (`tab` switches). `/agents runs` opens the Runs tab, and `/agents <name>` opens the form of that agent.

The Agents tab shows the list on the left and the form of the selected agent on the right (stacked on terminals narrower than 100 columns). Each list row has the name, a source badge (`built-in`, `global`, `project`, or `plugin <name>` for an agent that `claude-marketplace` staged), `✎ <scope>` when a file in that scope overrides a built-in or a lower file, the model, and the exposure. The form shows every field: name, description, model, thinking, tools, denied tools (`disallowedTools`), max turns, prompt mode, exposure, and prompt. Below it are the file path, the layers, the line that the `subagents` system prompt section shows for the agent, and its recent runs.

| Where | Keys |
| --- | --- |
| List | `↑`/`↓` move, `enter` edit, `n` new (from a template or as a copy), `c` copy, `r` rename, `m` move the file between global and project, `d` delete a custom agent or reset a built-in, `f` edit the raw file, `p` edit the prompt, `u` runs of this agent, `/` search, `q` close |
| Form | `↑`/`↓` field, `enter` edit, `←`/`→` cycle thinking, prompt mode, and exposure, `x` set the field to its default (inherit, all tools, none), `p` prompt, `s` save, `esc` back to the list |
| Model | type to search the available models (fuzzy), `enter` choose; text that matches no model is used as `provider/id` |
| Tools | type to search, `space` toggle, `ctrl+a` toggle the shown tools, the `all tools` row inherits every tool, `enter` done |
| Runs | `↑`/`↓` select, `enter` shows the task, the tool calls, the output, and the stderr of a failed run, `c` cancel, `a` all agents, `esc` back |

Text fields are typed in the form row. The prompt and the raw file open in pi's editor, whose external editor key opens `$EDITOR`. Edits stay in the form until `s` saves them. Leaving the form with changes asks to save, discard, or keep editing. A save writes only the changed keys to the agent's top file through a temp file and a rename. A built-in without a file asks for the scope (global or project) at the first save. Errors (an empty description, a model that is not `provider/id`, a bad max turns) block the save. Warnings (unknown tools, no tools, an empty prompt) show in the status line. The panel refuses to rename, move, or delete plugin agents; copy them instead, or remove them with `/claude-plugins`.

`/agents list`, and `/agents` without UI, show each agent with its source, description, tools, and model as a notification. `/agents runs` without UI shows the runs.

The system prompt section `subagents` lists each agent with the first sentence of its description when the `subagent` tool is active. `action: list` gives the full descriptions, tools, and models. On a pi with context entries, the section lists only the agents that pi does not list (see below).

## Deferred agents

This needs a pi-rust with [yannelli/pi-rust#226](https://github.com/yannelli/pi-rust/pull/226). The extension detects it when `initialize` has `contextEntries` in `features`. Older hosts keep the agent list in the `subagents` section.

On such a host, `resources_discover` returns one context entry per agent: `kind: "agent"`, the name, the description, the usage `Call the subagent tool with agent "<name>".`, and the `exposure` when the file sets it. Entries have no `path`, so the model calls the tool without reading the agent file. Set `contextExposure` in pi's settings, for example `{ "agent": "deferred" }`, to keep the agents out of the system prompt so that `tool_search` finds them. The `exposure` key of an agent file (`direct` or `deferred`) wins over the setting for that agent. Another value gives a warning on stderr and is ignored. With `advertise: false` the extension returns no entries.

The `subagents` section then keeps its usage text and lists only agents that pi does not have as entries: agents without a description, and agents made in `/agents` after the discovery. pi discovers entries only at startup and on `/reload`, and an extension cannot start a discovery. Run `/reload` after you change a description or `exposure`, or delete an agent, so that pi lists the change.

## Compactor active work

The extension publishes its running runs as the `[Active Work]` block `subagents` of the compactor (channel `compactor:active-work`): one line with run id, agent, and the first line of the task per run, at most 10. A finished, failed, or cancelled run leaves the block; no running run removes it. It publishes when a run starts or ends, when it adopts background runs, and again for each `compactor:active-work:sync` request.

## Config

`<agentDir>/ext/subagents.json`, then `<cwd>/.pi/ext/subagents.json` (project keys win; `piBin` and `extraArgs` apply from the project file only in a trusted project). In a git worktree without this file, the main checkout's copy applies.

| Key | Default | Use |
| --- | --- | --- |
| `asyncByDefault` | `false` | Background runs when the call has no `async` |
| `maxParallel` | `4` | Children that run at the same time per call |
| `maxTasks` | `8` | Tasks per call |
| `maxRecentRuns` | `20` | Finished runs kept for `/agents runs` and `status` |
| `widget` | `true` | Show the background runs widget |
| `advertise` | `true` | Add the `subagents` system prompt section |
| `piBin` | unset | pi binary for children |
| `extraArgs` | `[]` | Extra child arguments |
| `messaging` | `true` | The `agent_message` tool and mailbox delivery between agents |

## Not ported from pi-subagents

Workflows, missions, schedules, worktrees, the watchdog, supervisor and intercom tools, in-process child sessions, and custom `renderCall`/`renderResult`.
