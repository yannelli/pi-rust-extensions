# pi-ext-goal

Created 2026-10-30. Last updated 2026-10-31.

Persistent goals for pi. This is a Rust port of the `pi-goal-x` package (v0.34.1). It has guided drafting, a task tree, autonomous continuation turns, and a completion auditor. The tool names, the parameter names, and the command names are the same as upstream, so prompts written for pi-goal-x keep working. Run it as `pi-rust-ext goal`.

## Tools

| Tool | Parameters | Behavior |
| --- | --- | --- |
| `create_goal` | `objective`, `mode?` (`regular`/`sisyphus`), `token_budget?` | Creates and focuses a goal. Rejected during a draft. |
| `get_goal` | `section?` (`summary`/`objective`/`tasks`/`history`), `task_id?`, `cursor?`, `verbose?`, `include_history?` | Returns 4000-character pages. `cursor` is the offset from the previous page. |
| `update_goal` | `status?` (`complete`/`blocked`/`paused`), `continuation?` (`{kind:"ready"}` or `{kind:"wait", reason, deadline, wait_id?}`), `reason?`, `attempted_actions?`, `suggested_action?`, `completion_summary?` | Validation is the same as upstream `planUpdateGoal`. `complete` starts the auditor. A wait needs `strictExecutionContract` and an ISO 8601 deadline in the future. |
| `set_goal_tasks` | `tasks[]` (`id`, `title`, `parent_id?`, `verification_contract?`, `lightweight_subtasks?`), `block_completion?`, `change_summary?` | Replaces the task tree after the user confirms it. Tasks with a matching id keep their progress. |
| `update_goal_task` | `task_id?`, `status?` (`start`/`complete`/`skipped`/`pending`), `evidence?` (200 characters at most), `reason?` (required for `skipped`), `updates?[]` | Applies all the updates or none. A parent with open subtasks cannot complete unless it has `lightweight_subtasks`. |
| `goal_question` | `question`, `options?`, `recommended?`, `allow_custom?` | Only during a draft. Shows a select dialog with an "Other" choice. |
| `goal_questionnaire` | `questions[]` (`id`, `question`, `context?`, `options`, `recommended?`, `allow_custom?`) | Only during a draft. Asks the questions in sequence. |
| `propose_goal_draft` | `objective`, `token_budget?` (`null` removes it), `auto_continue?`, `sisyphus?`, `tasks?`, `block_completion?` | Only during a draft. Opens a scrollable panel: Enter confirms, `c` or Esc continues the chat, and `x` cancels the draft. |

Tool profile: on each `before_agent_start`, the extension puts the profile tools in the active tool list. The profile is the three core tools plus the two task tools (unless `disableTasks` is set), or the three drafting tools while a draft is open. The other active tools do not change. Without a UI, or in `rpc` mode, a proposal is confirmed automatically and a question gets its recommended option.

## Commands

`/goal [idea]`, `/sisyphus [idea]`: start a guided draft. `/goal-direct <objective>`, `/sisyphus-direct`: create a goal and start it. `/goal-tweak [change]`: revise the focused goal through a draft. `/goal-pause` (stops the running turn), `/goal-resume [id]` (renews the run allowance), `/goal-clear` (archives the goal after you confirm), `/goal-cancel` (cancels the draft). `/goal-list`, `/goal-status [verbose|health]`, `/goal-refresh`, `/goal-recovery [repair]`, `/goal-focus [id]`, `/goal-unfocus`. `/goal-settings` opens a menu that changes the settings in the global or the project file. Shortcut `ctrl+shift+t` expands or collapses the widget.

## Runtime

- The system prompt section `goal` holds the policy, the objective, up to three pending tasks, and the limits. With no focused goal, it holds the upstream `[PI GOAL UNFOCUSED]` reminder.
- At `agent_settled`, the extension sends a `pi-goal-event` custom message with `triggerTurn` when the focused goal is active and has auto-continue. The message content is the upstream `<pi_goal_continuation .../>` marker. An aborted turn pauses the goal. Three runs in a row without a tool call also pause it. The token budget and `maxAutonomousRuns` stop the continuation.
- The auditor runs `llm.complete` with the upstream audit checklist, the task tree, `git status --short`, `git diff --stat`, and the end of the transcript. `update_goal` waits for the verdict and returns it as its tool result, as upstream does, so the model gets the findings in the same run, also in print mode. The last line `<approved/>` archives the goal. Any other result sets the goal back to active, and the tool result starts with `[GOAL AUDIT REJECTED` and the findings.
- A pause, clear, Esc, or newer completion request while the auditor runs makes `update_goal` return "The audit stopped before a verdict" and drops the late verdict. When the tool call itself is gone (the turn ended), the verdict goes out as a `pi-goal-audit-event` message instead, and a rejection starts a rework turn.
- After a goal stops in a turn, `tool_call` blocks all tools except `get_goal` until the next turn.
- The `pi-goal:wake` event channel (`{ goalId? }`) ends a wait at once.
- The widget `goal` above the editor shows the status, the time, the tokens, and the open tasks. The status key `goal` shows a draft or an unfocused goal.

## Compactor active work

The extension publishes the focused open goal as the `[Active Work]` block `goal` of the compactor (channel `compactor:active-work`): objective, status, the blocked reason, a wait, the current task, and up to 8 open tasks. A draft shows as "drafting a new goal". A complete goal, or no focused goal, removes the block. It publishes on each change, also without a UI, and again for each `compactor:active-work:sync` request.

## Settings

Layers, lowest to highest: `<agentDir>/pi-goal-x-settings.json`, `<agentDir>/ext/goal.json`, the `goal` settings namespace, `<cwd>/.pi/pi-goal-x-settings.json`, `<cwd>/.pi/ext/goal.json`. Project files and the merged settings namespace set `provider`, `model`, and `thinkingLevel` only in a trusted project. In other projects, the extension keeps the global values and reports ignored project overrides once.

| Key | Default | Use |
| --- | --- | --- |
| `disableTasks` | `false` | Removes the task tools. |
| `disableContracts` | `false` | Leaves verification contracts out of the prompts. |
| `disableAuditor` | `false` | Completes the goal without an audit. |
| `maxAutonomousRuns` | unlimited | Continuation runs for each start or resume. `0` turns continuation off. |
| `showAutonomousRuns` | `true` | Shows the run allowance in the prompt. |
| `strictExecutionContract` | `false` | Lets the agent save wait continuations. |
| `hideUnfocusedPrompt`, `hideUnfocusedBanner` | `false` | Hide the unfocused reminder in the prompt and in the status line. |
| `objectiveMaxChars` | none | Sets a length limit for objectives. |
| `provider`, `model`, `thinkingLevel` | current model | The auditor model. `model` can also be `provider/id`. |
| `goalsRoot` | unset | A goal file directory relative to the project (`~/` expands). |

## Paths

- Goal files: `<agentDir>/ext/goal/projects/<cwd slug>/<id>.json`, or `<goalsRoot>/<id>.json`. Approved and cleared goals move to `archive/<id>.json`. `/goal-recovery repair` moves unreadable files to `archive/<name>.broken`. Writes use a temporary file and a rename. The file format is JSON, and upstream uses markdown files and a ledger.
- Session entries: `pi-goal-focus` (`{ goalId }`) and `pi-goal-draft` (an entry with `clearedAt` cancels the draft). The extension reads them again on `session_start`, `session_tree`, and `session_compact`.

## Differences from upstream

The extension does not have the ledger, the file locks, the prompt-cache payload rewrite, the network-error backoff, wait polling, the blocker oracle, keybinding settings, or command renames. The auditor is a single `llm.complete` call, and upstream runs a tool-using sub-session.

## Tests

`cargo test -p pi-ext-goal`: unit tests in `src/tests.rs` and `src/tests_io.rs`, protocol tests in `tests/protocol.rs`, and an end-to-end run under pi in `tests/e2e.rs`.
