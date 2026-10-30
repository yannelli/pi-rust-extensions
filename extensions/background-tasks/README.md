# background-tasks

Created 2026-10-30. Last updated 2026-10-30.

Durable background shell jobs for pi, ported from the core of `pi-background-tasks` (v2.6.10). The agent starts long-running commands (tests, builds, dev servers, watchers) without blocking a turn. A notification reports the end state. Jobs keep running across `/reload`. Fusion, delegated agents, attested runs and Anthropic attribution are not ported.

## Tools

Tool and parameter names match upstream, so prompts written for it keep working.

| Tool | Parameters | Result |
| --- | --- | --- |
| `bg_run` | `name`, `command`, `isAgent`, `description?`, `timeoutSeconds?`, `notifyOnCompletion?` (default true), `triggerOnCompletion?` (default true), `surviveReload?` (default true) | Task id and output path, at once |
| `bg_status` | `taskId?` (id or unique prefix) | One task, or all tasks of the session |
| `bg_logs` | `taskId`, `maxBytes?` (default and cap 50KB), `tail?` (default true) | Bounded output with the full output path when truncated |
| `bg_kill` | `taskId` | Stops the process group: SIGTERM, then SIGKILL after 3 seconds |

When a task completes, fails, times out or is lost, the extension sends a `background-task-notification` custom message (`deliverAs: followUp`). It starts a turn when `triggerOnCompletion` is true. Tasks stopped by `bg_kill`, `/kill`, `/tasks` or session shutdown do not send a notification.

`before_agent_start` adds the system prompt section `pi_background_shell_policy`.

## Commands

| Command | Action |
| --- | --- |
| `/bg [--no-survive-reload] [--agent] [--name "Task name"] <command>` | Starts a task. Its notification does not start a turn. |
| `/tasks [id]`, `/bg-tasks [id]` | Task manager panel: `↑↓`/`jk` select, `enter` logs, `x` twice stops, `c` removes finished tasks, `esc`/`q` closes or goes back. Refreshes every second. |
| `/jobs` | Lists tasks. |
| `/logs <id> [maxBytes]` | Shows the output tail. Completes task ids. |
| `/kill <id>` | Stops a running task. Completes running task ids. |
| `/bg-clear` | Removes finished tasks and their output files. |

The footer status `background-tasks` shows counts by state (`bg: 1 running, 2 completed`). A widget below the editor lists up to 4 running tasks with elapsed time.

## Reload and shutdown

Each task runs detached in its own process group. Its stdout and stderr go to a file, and a wrapper shell writes the exit code to a file. A new extension process reads the task files at `session_start` and adopts the tasks. It then finds exit codes, dead processes and timeouts.

- `session_shutdown` with reason `reload`: tasks with `surviveReload: true` keep running; the others stop.
- Reasons `quit`, `new`, `resume`, `fork`: all running tasks stop.
- When pi exits without `session_shutdown`, tasks keep running and the next session start with the same session id adopts them.

Upstream defaults `surviveReload` to false. This port defaults it to true because reload survival is the main feature.

## Paths

- Task state: `<agentDir>/ext/background-tasks/sessions/<sessionId>/<taskId>/task.json`
- Output: `.../<taskId>/output.log`
- Exit code: `.../<taskId>/exit`

On unix, the extension creates these directories with mode 0700 and these files with mode 0600.

At spawn the extension records the start time of the task's process (`ps -o lstart=`, `LC_ALL=C`) as `processStart`. Before each signal it compares the start time of the pid with `processStart`. When the pid is gone, belongs to another process, or has no readable start time, the extension sends no signal and marks the task `lost` (or `completed`/`failed` when the exit file exists). After TERM, KILL goes to the group only when the leader still matches or a group member seen before TERM is still in the group. Task records without `processStart` become `lost` when their pid is alive.

The extension has no config file. Commands run with `/bin/bash -c` (or `/bin/sh -c` without bash) in the session cwd. On Windows they run with `cmd /C` and stop with `taskkill /T /F`.

## Tests

`cargo test -p pi-ext-background-tasks` runs unit tests (parsing, prefix resolution, bounded reads, state, panel keys, process lifecycle, recovery, timeout, TERM-ignoring descendants, shutdown rules), protocol tests against an in-memory host, and an e2e test under the dev pi binary (`PI_BIN`; it skips when the binary is missing).
