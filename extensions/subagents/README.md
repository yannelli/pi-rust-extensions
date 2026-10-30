# pi-ext-subagents

Created 2026-10-30. Last updated 2026-10-30.

Runs tasks in child pi processes. Three built-in agents: `explore` (read-only search), `build` (full tools), and `advisor` (read-only review with a recommendation). You can change them and add your own. Run it with `pi-rust-ext subagents`. Simplified from `pi-subagents`.

## Tool `subagent`

| Param | Use |
| --- | --- |
| `action` | `run` (default), `list` (agents), `status` (all runs, or one run by `id`), `stop` (`id`) |
| `agent` | Agent name. Default `explore`. |
| `task` | The full task. The child does not see the parent conversation. |
| `tasks` | `[{ agent?, task, model?, thinking?, cwd? }]`: tasks that run in parallel (`maxParallel` at a time). |
| `async` | `true` runs in the background. The tool returns the run ids at once. A `subagent-result` message comes when the runs finish (`deliverAs: followUp`, `triggerTurn: true`). |
| `model`, `thinking` | Override for this call. |
| `cwd` | Child working directory, relative to the session cwd. A path that resolves outside the session cwd is refused. |

Foreground runs send `tool.update` with each child tool call and the tail of the child's text. Background runs show in the widget `subagents` below the editor. The result `details` is `{ runs: [{ id, agent, task, status, background, model, turns, tools, usage, output, exitCode, durationMs }] }`. An abort of the tool call kills the child's process group. On shutdown the extension cancels all runs and waits up to 1.4 seconds, so the group gets SIGKILL 1 second after SIGTERM.

## Child process

`<pi> --mode json -p --no-session [--model <m>] [--thinking <t>] (--tools <list> | --no-tools | --exclude-tools subagent[,<disallowedTools>]) (--append-system-prompt | --system-prompt) <prompt> [--max-turns <n>] [extraArgs] -- <task>`, in the session cwd.

- Model and thinking level: the call override, else the agent's `model`, else the parent session's current model. When neither the call nor the agent sets a model, the child also gets the parent's thinking level, unless the agent or call sets one.
- The pi binary: config `piBin`, else `$PI_BIN`, else the parent process when its file name starts with `pi`, else `pi` on `PATH`.
- Env: `PI_TODO_SCOPE=<run id>`, `PI_SUBAGENT_RUN_ID=<run id>`, `PI_SUBAGENT_AGENT=<name>`.
- Todo scopes: each child `todo` tool result is sent on the `pi.events` channel `todo:scope` as `{ scope, label, status, state }`, with `status` `running`, `completed`, `failed`, or `cancelled` and `state` the child's todo `details` (or `null`). The `todo` extension shows these scopes.
- Lifecycle: `subagents:start` `{ agentId, agentType }` when a run starts and `subagents:stop` `{ agentId, agentType, status, lastAssistantMessage }` when it ends, on `pi.events`. The `hooks` extension runs SubagentStart and SubagentStop hooks from them.

## Agent files

Built-in defaults, then `<agentDir>/ext/subagents/agents/*.md`, then `<cwd>/.pi/ext/subagents/agents/*.md`. A later file overrides only the fields it sets. A file with a new name adds an agent. Project agents load only when the project is trusted. The format is the Claude Code agent format (YAML frontmatter and a prompt body), so `claude-marketplace` writes plugin agents here.

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
---
You review code...
```

## Command `/agents`

- `/agents`: a menu with each agent. For an agent you can edit the prompt (in the editor), the description, model (session model, available models, or typed), thinking level, tools, and max turns. You can reset a built-in to its default (removes the override files) and delete a custom agent. The menu asks for the scope (global or project) when an agent has no file yet.
- `/agents runs`: a panel with running and recent runs. `↑`/`↓` select, `enter` shows the task, the tool calls, the output, and the stderr of a failed run, `c` cancels, `esc` goes back, `q` closes.
- `/agents list`, or `/agents` without UI: shows the agent list as a notification.

The system prompt section `subagents` lists the agents when the `subagent` tool is active.

## Config

`<agentDir>/ext/subagents.json`, then `<cwd>/.pi/ext/subagents.json` (project keys win; `piBin` and `extraArgs` apply from the project file only in a trusted project).

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

## Not ported from pi-subagents

Workflows, missions, schedules, worktrees, the watchdog, supervisor and intercom tools, in-process child sessions, and custom `renderCall`/`renderResult`.
