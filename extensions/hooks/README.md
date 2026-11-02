# hooks

Created 2026-10-29. Last updated 2026-11-02.

Runs Claude Code and Codex CLI hooks from pi. It reads the hook configs of both tools and a pi-native hooks file, maps pi events onto hook events, runs the matching handlers, and applies their decisions to pi. It follows the Claude Code hooks reference (code.claude.com/docs/en/hooks) and the Codex hooks docs (developers.openai.com/codex/hooks), read 2026-11-02.

## Commands and flags

| Name | Use |
| --- | --- |
| `/hooks` | Opens the hooks manager panel. Without a UI it shows the listing as a notification. |
| `/hooks list` | Shows the listing as a notification. |
| `--hooks-trust-all` | Runs every hook without a trust prompt for this run (like Codex `--dangerously-bypass-hook-trust`). |
| `--init`, `--init-only`, `--maintenance` | Run Setup hooks, as the Claude flags do. `--init` and `--maintenance` work in print mode (no UI). `--init-only` runs Setup and SessionStart hooks, then shuts pi down. |
| `--worktree <name>` | Starts in a new worktree: WorktreeCreate hooks, else `git worktree add -b <name> <git root>/.pi/worktrees/<name>`. Needs a pi host that lets `session_start` handlers switch sessions. |
| `/worktree [name]` | Creates a worktree the same way and switches to a new session in it. `/worktree exit [--force]` switches back and removes it (WorktreeRemove hooks, else `git worktree remove`). On quit, a clean worktree from `/worktree` is removed. |
| `/add-dir <path>` | Adds a working directory: tells the model, then runs DirectoryAdded hooks (`source: "slash_command"`) in the background. Their `systemMessage` goes to the model on the next turn. |

### Hooks manager

The panel lists every hook handler grouped by source: Claude Code (user `settings.json`, project `.claude/settings.json`, local `.claude/settings.local.json`), Codex (`hooks.json` and inline `config.toml` hooks), pi (`ext/hooks.json` files), and Plugins (pi-native groups with a `plugin` id). Each row shows the enabled state (`●` on, `○` off), event, matcher, scope (`global`, `project`, or `local`), trust state, and the command. Below 72 columns the scope and trust state shrink to two characters, such as `G✓` (global, trusted) or `L?` (local, untrusted).

| Key | Action |
| --- | --- |
| Up/Down, Home/End, PageUp/PageDown | Move. Up and Down wrap. |
| Space | Enable or disable the selected hook. |
| Enter | Details: full command, type, timeout, `async`/`asyncRewake`/`once` flags, status message, file path, plugin, trust, and last run with stdout and stderr. The details view lists actions: Enable/Disable, Trust always, Allow once this session, Untrust, Deny, Remove from file. Up/Down choose, Enter runs, Esc goes back. |
| `d` or Delete | Remove the hook from its file after a `y`/`n` confirm. |
| `a` | Add a command hook: `p` project or `g` global, then the event, matcher, and command. |
| Tab, Shift+Tab | Switch the All, Project, and Global tabs. |
| `/` | Search event, matcher, command, source, scope, file, and plugin id. Enter keeps the filter; Esc clears it. |
| `s`, `c`, `t` | Turn a source on or off, set the `scope` option, and toggle `trustAll`, in the global options file. |
| Esc or `q` | Close (Esc clears an active search first). |

Writes per scope:

- Disable and enable add or remove the hook hash in the `disabled` list of the pi options file of the hook's scope: `<agentDir>/ext/hooks.json` for global hooks, `<cwd>/.pi/ext/hooks.json` for project and local hooks. The hook definition stays in its file. Enable also clears the hash from the older `state.json` `disabled` list.
- Remove deletes the handler from its own file (Claude settings, Codex `hooks.json` or `config.toml`, or the pi file), then drops a matcher group or event that became empty. JSON files keep their key order, unknown keys, indent, and trailing newline; `config.toml` keeps comments and formatting (`toml_edit`). A file that does not parse is not changed. Remove also clears the hash from the trust and disabled lists.
- Add appends `{ matcher?, hooks: [{ type: "command", command }] }` to the pi file of the chosen scope and marks the new hook trusted.
- Project scope needs a trusted project (`isProjectTrusted`). In an untrusted project the panel shows a warning, hides project hooks, and refuses to add project hooks.

Each change reloads the config, so the running session uses it at once.

The extension registers no tools.

## Sources

| Source | Global | Project |
| --- | --- | --- |
| Claude | `$CLAUDE_CONFIG_DIR/settings.json`, else `~/.claude/settings.json` | `<root>/.claude/settings.json`, `<root>/.claude/settings.local.json` |
| Codex | `/etc/codex` (Unix), then `$CODEX_HOME` (else `~/.codex`): `hooks.json` and `config.toml` | Each `.codex/hooks.json` and `.codex/config.toml` from the git root down to the cwd |
| pi | `<agentDir>/ext/hooks.json` | `<cwd>/.pi/ext/hooks.json` |

- `<root>` is the session cwd when it holds `.claude` (or `.codex`), else the nearest parent directory with `.git`, else the cwd. `CLAUDE_PROJECT_DIR` is the Claude `<root>`.
- Hooks from all files merge.
- Project files of all three sources load only when the ctx snapshot says `isProjectTrusted: true`. Otherwise the extension skips them and shows one warning per session that lists the skipped files. Options in an untrusted project `.pi/ext/hooks.json` are skipped too.
- `disableAllHooks` follows Claude settings precedence: the value in the most specific loaded file wins (local, then project, then user). A project `false` overrides a user `true`. When it is `true`, pi-native groups with a `plugin` id are off too, because they come from Claude Code plugins.
- Codex `[features] hooks = false` (or `codex_hooks = false`) turns off the Codex source. The most specific layer that sets it wins.
- Codex `[hooks.state."<file>:<snake_event>:<group>:<handler>"] enabled = false` in the user `config.toml` turns off one Codex hook, as Codex `/hooks` does. `trusted_hash` is not read; pi uses its own trust list.
- Codex inline hooks in `config.toml` (`[[hooks.<Event>]]` with `[[hooks.<Event>.hooks]]`) merge with `hooks.json` of the same layer, with a log warning.
- pi-native files use the Claude settings shape and Claude semantics. Matcher groups can carry `plugin`, `pluginRoot`, and `pluginData` (written by `claude-marketplace`). The hook gets `CLAUDE_PLUGIN_ROOT`, `PLUGIN_ROOT`, `CLAUDE_PLUGIN_DATA`, and `PLUGIN_DATA` from them, and `/hooks` shows the plugin id.

## Options

In `<agentDir>/ext/hooks.json` and `<cwd>/.pi/ext/hooks.json` (same file as the pi-native hooks). The project file wins per key; `sources` merges per sub-key. `/hooks` writes these keys and keeps all other keys.

| Key | Default | Use |
| --- | --- | --- |
| `sources.claude`, `sources.codex`, `sources.pi` | `true` | Turn a source on or off. |
| `scope` | `"both"` | `global`, `project`, or `both`: which files load. |
| `trustAll` | `false` | Run all hooks without trust prompts. Read from the global options file only; a project file's `trustAll` is ignored. |
| `disabled` | `[]` | Hashes of disabled hooks. The lists of both files merge. |

## Trust

Each hook has a hash: sha256 hex of the canonical JSON (sorted keys) `{ file, event, matcher, handler }`, with `file` the absolute config path and `matcher` `null` when omitted. A changed handler or a moved file gives a new hash.

- With `trustAll` or `--hooks-trust-all`, all hooks run except denied ones.
- Otherwise the first run of an untrusted hook asks with "Allow once", "Always allow", "Deny". "Always allow" and "Deny" go to `state.json`; "Allow once" lasts for the session. A dismissed prompt skips the hook this time.
- Without a UI, untrusted hooks are skipped with a log line.
- Disabled hooks do not run.
- The extension resolves trust for all matched handlers first, then runs them in parallel.

## Events

| pi event | Hook events | Effect |
| --- | --- | --- |
| `session_start` | SessionStart (`startup`, `resume` for reload and resume, `clear` for new, `fork`) | Reloads config. `additionalContext` and plain stdout are held for the next `before_agent_start`. `sessionTitle` sets the session name. `initialUserMessage` is sent as a user message only on `startup` without a UI (print mode). |
| `session_shutdown` | SessionEnd (`prompt_input_exit` for quit, `clear`, `resume`, else `other`; Codex `other`) | Advisory. |
| `input` (interactive and rpc) | UserPromptSubmit | Block (exit 2, `decision: "block"`, `continue: false`) answers `handled` and shows the reason. Context transforms the text to `text + "\n\n" + context`. Resets `stop_hook_active`. |
| `before_agent_start` | none | Delivers held context once as `{ message: { customType: "hooks-context", content, display: false } }`. |
| `tool_call` | PreToolUse, then PermissionRequest (unless PreToolUse allowed or denied) | Deny gives `{ block: true, reason }`. Precedence: deny > defer > ask > allow. `ask` asks with `ui.confirm`; without a UI the call is blocked. `updatedInput` replaces the whole input (the patch removes keys the hook dropped). `additionalContext` is appended to the tool result. Claude `continue: false` blocks with `stopReason` and aborts the run. Codex `allow` with `updatedInput` rewrites the input but does not approve; Codex `updatedInput` for `apply_patch` (pi edit and write) is ignored with a log line. |
| `tool_result` | PostToolUse, or PostToolUseFailure when `isError` (Claude and pi; Codex gets PostToolUse) | Claude `decision: "block"` appends the reason; Codex replaces the content with it. Exit 2 stderr and `additionalContext` are appended. Claude `updatedToolOutput` replaces the content (a string becomes a text block, other values become JSON text). Claude `continue: false` appends `stopReason` and aborts the run. Codex `continue: false` and Codex exit 2 replace the content with the reason, and the run continues. |
| `turn_end` with tool results | PostToolBatch (Claude and pi) | `additionalContext` is sent as a `hooks-context` steer message (`display: false`) before the next model call. A block or `continue: false` shows the reason, keeps it as a `hooks-feedback` message for the next turn, and aborts the run. |
| `agent_end` | none | Keeps the last assistant text and whether it ended with an error. |
| `agent_settled` | Stop; StopFailure (Claude and pi) after an error; Codex Interrupt when aborted | A Stop block sends `hooks-stop` with the reason (`display: false`, `deliverAs: followUp`, `triggerTurn: true`) and sets `stop_hook_active` until the next user input. Claude and pi `additionalContext` continue the turn in the same way. After 8 consecutive continuations (`CLAUDE_CODE_STOP_HOOK_BLOCK_CAP`), the next one is skipped and the turn ends; a tool call or user input resets the count. `continue: false` from any hook wins. |
| `session_before_compact` | PreCompact (`manual`, else `auto`) | Block or `continue: false` cancels the compaction. |
| `session_compact` | PostCompact, then SessionStart with source `compact` | Context is held. |
| `model_select` | PreModelSwitch, then PostModelSwitch (Claude and pi) | pi sends `model_select` after the switch, so a block switches back to the previous model and shows the reason. Exit 2, `decision: "block"`, `permissionDecision: "deny"`, `continue: false`, and a timeout block. `ask` asks with `ui.confirm`; without a UI it blocks. PreModelSwitch does not run for a `restore` switch. PostModelSwitch context is held. |
| `before_provider_request`, `after_provider_response` | Codex PreModelRequest, PostModelRequest | Diagnostic only; the request does not wait. PostModelRequest `status` is `completed` for a 2xx HTTP status, else `failed` with `error: "HTTP <status>"`; `token_usage` is `null`. |
| `pi.events` subagent channels | SubagentStart, SubagentStop; `subagent:async-complete` also gives Notification `agent_completed` and TeammateIdle (`team_name: "subagents"`) | See below. A TeammateIdle block is emitted on `hooks:teammate-idle-blocked` `{ agentId, reason }`. |
| `tool_call` of the `todo` tool | TaskCreated for each task of `action: "create"`, TaskCompleted for `action: "update"` with `status: "completed"` (after PreToolUse and PermissionRequest) | A block (exit 2 or `decision: "block"`) blocks the `todo` call with the reason. `task_id` continues from the last `todo` result `nextId`; the subject of a completed task comes from the last `todo` result. `owner` is sent as `teammate_name`. |
| A 1-second watcher | FileChanged, ConfigChange, Notification `idle_prompt` | FileChanged watches the literal file names of FileChanged matchers in the cwd plus `watchPaths` from SessionStart, CwdChanged, and FileChanged (`event`: `add`, `change`, `unlink`). ConfigChange watches the Claude user, project, local, and managed settings files and skill files (`source`: `user_settings`, `project_settings`, `local_settings`, `policy_settings`, `skills`); without a block it reloads the hooks config, so edits apply at once. `idle_prompt` fires 60 seconds after the agent settled without new input (UI only). |
| `ui.confirm` of a hook `ask` | Notification `permission_prompt` | Fires when the dialog stays open for 6 seconds. |
| `instructions_loaded` (pi-rust host event) | InstructionsLoaded (`load_reason: "session_start"`, `memory_type`: `User` or `Project`) | Advisory, runs in the background. |
| `prompt_expansion` (pi-rust) | UserPromptExpansion (matcher: `command_name`) | A block stops the prompt and shows the reason; context and plain stdout are added to the expanded prompt. |
| `notification` (pi-rust) | Notification `plugin_notification` (extension `ui.notify`, `title` is the extension) or `pi_notification` (pi's own notices) | Notices from this extension are skipped. |
| `tool_call_blocked` (pi-rust) | PermissionDenied (blocks by other extensions or pi's permission check, not by this extension) | `retry: true` sends a `hooks-retry` steer message that tells the model it may retry. |
| `cwd_changed` (pi-rust) | CwdChanged (`old_cwd`, `new_cwd`) | Clears `CLAUDE_ENV_FILE`; `watchPaths` replaces the FileChanged list. |
| `message_display` (pi-rust) | MessageDisplay (`turn_id`, `message_id`, `index`, `final`, `delta`) | `displayContent` replaces the shown text; the session keeps the original. |
| `mcp_elicitation`, `mcp_elicitation_result` (pi-rust) | Elicitation, ElicitationResult (matcher: server name) | `action` (`accept` with `content`, `decline`, `cancel`) answers or changes the reply; a decline, exit 2, or `decision: "block"` wins. ElicitationResult also gives Notification `elicitation_response`. |

Matcher targets: tool name for tool events, `source` for SessionStart, `reason` for SessionEnd, `trigger` for Pre/PostCompact, agent type for Subagent events, error type for StopFailure (`rate_limit`, `overloaded`, `authentication_failed`, `billing_error`, `max_output_tokens`, `model_not_found`, `server_error`, `invalid_request`, `unknown`, from the error text), model id for Pre/PostModelSwitch, `trigger` for Setup, `load_reason` for InstructionsLoaded, `command_name` for UserPromptExpansion, `notification_type` for Notification, `source` for ConfigChange and DirectoryAdded, the file name for FileChanged, server name for Elicitation and ElicitationResult. UserPromptSubmit, PostToolBatch, Stop, Interrupt, TaskCreated, TaskCompleted, TeammateIdle, CwdChanged, MessageDisplay, PreModelRequest, and PostModelRequest ignore the matcher.

### Subagent channel contract

The extension subscribes to `subagents:start` and `subagent:async-started` (SubagentStart), and `subagents:stop`, `subagent:async-complete`, `subagent:foreground-complete` (SubagentStop). Data: `{ agentId, agentType, sessionId?, lastAssistantMessage? }`; `id`/`runId`, `agent`/`type`, and `result`/`output` are read as fallbacks. SubagentStart context is emitted on `hooks:subagent-context` `{ agentId, additionalContext }`. A SubagentStop block is emitted on `hooks:subagent-stop-blocked` `{ agentId, reason }`.

## Matchers

- Claude and pi: `*`, `""`, or omitted match all. A matcher of only `[A-Za-z0-9_\- ,|]` is an exact name or a `|`/`,` list. Other matchers are unanchored JavaScript regular expressions (`regress`, ECMAScript syntax with lookaround and backreferences): `Edit.*` also matches `NotebookEdit`, `^(?!mcp__)` matches every tool that is not an MCP tool.
- Claude StopFailure: the exact-name set is only `[A-Za-z0-9_|]`, so a hyphen, space, or comma makes the matcher a regex.
- Codex: `*` or `""` match all. A matcher of only `[A-Za-z0-9_|]` is an exact name or a `|` list; other matchers are unanchored regexes. Tool matchers test the Codex name and its aliases: `apply_patch` (pi edit and write) also as `Edit` and `Write`.
- An invalid regex matches nothing and is logged.
- `if` (Claude, tool events only; a hook with `if` on another event never runs): one rule such as `Bash(git *)` or `Edit(*.ts)`. The tool name must match, and the pattern (`*` is a wildcard, a trailing `:*` is a prefix) must match the whole primary input: Bash `command`, Read/Edit/Write/LS `file_path` (absolute, or relative to the cwd), Glob/Grep `pattern`.
- Bash `if` rules test each subcommand: the command is split on `&&`, `||`, `;`, `|`, `&`, and newlines outside quotes, `$()` and backtick bodies are added, and leading `VAR=value` words are removed. A pattern with more than the command name (`Bash(git push *)`) runs the hook for any command with `$`, `$()`, or backticks. Unclosed quotes run the hook.

Tool names: `bash` -> `Bash`, `read` -> `Read`, `edit` -> `Edit`, `write` -> `Write`, `grep` -> `Grep`, `find` -> `Glob`, `ls` -> `LS`, `powershell` -> `PowerShell`; other names (MCP tools too) are unchanged. `tool_input` adds an absolute `file_path` (`~` and relative paths resolved against the cwd) next to `path` for read/edit/write, and `old_string`/`new_string` for edit (top level for one edit, and in each `edits` item). `updatedInput` maps back to pi names; `file_path` wins over `path`.

Codex tool input: pi edit and write reach Codex hooks as `tool_name: "apply_patch"` with `tool_input: { command }`, an `*** Begin Patch` text (`*** Add File` for write, `*** Update File` with one `@@` hunk per edit for edit). Codex PostToolUse `tool_response` is the result text.

## Stdin

All events: `session_id`, `transcript_path` (session file or `null`), `cwd`, `hook_event_name`, `permission_mode: "default"`. Claude and pi also get `prompt_id` (a new id for each user input, absent before the first), `effort: { level }` on tool events, Stop, and SubagentStop when a thinking level is set (`minimal` and `low` are `low`), `session_title` on SessionStart and UserPromptSubmit when the session has a name, and `mcp_server: { name, source }` on tool events of `mcp__<server>__<tool>` tools (`source` from `pi.getMcpServers()`, else `user`). Codex also gets `model` (the model id) and, on UserPromptSubmit, PreToolUse, PermissionRequest, PostToolUse, Stop, PreCompact, PostCompact, SubagentStart, SubagentStop, Interrupt, PreModelRequest, and PostModelRequest, `turn_id`. Tool events: `tool_name`, `tool_input`, `tool_use_id`; PostToolUse adds `tool_response` `{ content, text, isError, details }`, PostToolUseFailure adds `error` and `is_interrupt` (`true` for an aborted call). PostToolUse and PostToolUseFailure add `duration_ms`, measured from the end of `tool_call` to `tool_result`. PostToolBatch: `tool_calls`, each `{ tool_name, tool_input, tool_use_id, tool_response }` with the result text. Stop: `stop_hook_active`, `last_assistant_message`, `background_tasks` (`[{ source, description }]` from the `compactor:active-work` channel that background-tasks and subagents publish), `session_crons: []`; StopFailure adds `error` and `error_details`. UserPromptSubmit: `prompt`. SessionStart: `source`, `model`; on `resume` and `fork` with an assistant response, `seconds_since_last_response`, `context_tokens`, `prompt_cache_likely_expired` (300-second cache), `estimated_cache_write_usd`. SessionEnd: `reason`. Pre/PostCompact: `trigger`; PreCompact `custom_instructions`; PostCompact `compact_summary`. Pre/PostModelSwitch: `from_model`, `to_model`, `requested_model` (`null` on `resume`), `source` (`command` for a set, `picker` for a cycle, `resume` for a restore, else `sdk`). PreModelSwitch and PostModelSwitch also get `context_tokens`, `prompt_cache_warm` (a response in the last 300 seconds), `cache_ttl: "5m"`, `estimated_cache_write_usd`, and `pricing` (`catalog` from the model's `cost.cacheWrite`, else `default` at 3.75 USD per million tokens). PostModelRequest: `status`, `error`, `token_usage`. Subagent events: `agent_id`, `agent_type`; SubagentStop adds `stop_hook_active`, `last_assistant_message`, and `agent_transcript_path` (`sessionFile` or `transcriptPath` of the channel data).

## Output rules

- Stdout counts as JSON only when the trimmed text starts with `{`, ends with `}`, and parses.
- Exit 0: JSON is applied. Text that starts with `{` and ends with `}` but does not parse is a non-blocking error and is not used as context.
- Exit 2: blocks on PreToolUse, UserPromptSubmit, Stop, SubagentStop, PostToolBatch, PreCompact, PreModelSwitch (Codex: PreToolUse, PermissionRequest (deny), PostToolUse (replaces the result), UserPromptSubmit, Stop, SubagentStop). The reason is the reason of a blocking JSON decision, else stderr. Claude JSON is still read but cannot override the block. Claude and pi PostToolUse exit 2 stderr goes to the model. A Codex exit 2 with empty stderr is a non-blocking error. A Claude Stop or SubagentStop exit 2 with empty stdout and `No such file or directory` on stderr is a non-blocking error.
- Other exit codes are non-blocking errors; Claude JSON still applies.
- PreToolUse exit 2 reason (Claude and pi): `PreToolUse:<Tool> hook error: [<command>]: <stderr>`.
- Claude and pi `permissionDecision` is read under `hookSpecificOutput` and at the top level. Legacy `decision: approve|block` maps to allow and deny.
- Codex reads `permissionDecision` under `hookSpecificOutput` only. These outputs mark the run failed, and the action continues: on PreToolUse `ask`, `allow` without `updatedInput`, `updatedInput` without `allow`, `deny` without a reason, `decision: "approve"`, `continue: false`, `stopReason`, `suppressOutput`; on PermissionRequest `updatedInput`, `updatedPermissions`, `interrupt: true`, `continue: false`, `stopReason`, `suppressOutput`; on PostToolUse `updatedMCPToolOutput`, `suppressOutput`; `decision: "block"` without a reason on any event. Codex PreCompact reads only the universal fields.
- Plain stdout is context only for UserPromptSubmit, SessionStart, PostModelSwitch (Claude and pi) and SessionStart, UserPromptSubmit, SubagentStart (Codex). Codex Stop and SubagentStop need JSON.
- Universal JSON: `continue: false` stops and wins over decisions; `systemMessage` shows a warning; `suppressOutput` has no effect; `terminalSequence` is written to the terminal when it is BEL or OSC 0/1/2/9/99/777.
- Claude and pi: `additionalContext`, `systemMessage`, `initialUserMessage`, and plain stdout are capped at 10,000 characters each. Longer text goes to `<tmp>/hook_outputs/<session_id>/<id>.txt` and is replaced by the path and the first 2,000 characters.
- Codex `additionalContextLimit` (tokens, default 2500, about 4 characters per token, 0 for no limit) applies to context, feedback, block reasons, and `systemMessage`. Longer text goes to the same directory and is replaced by the path and a head and tail preview within the limit.

## Handlers

- `command`: `sh -c <command>` (Codex: `$SHELL -lc <command>`, else `/bin/sh -lc`; `cmd /d /s /c` on Windows), or exec form without a shell when `args` is set. In exec form, `${CLAUDE_PROJECT_DIR}`, `${CLAUDE_PLUGIN_ROOT}`, and `${CLAUDE_PLUGIN_DATA}` in `command` and `args` are replaced as plain strings. Stdin is the event JSON; the cwd is the session cwd. `timeout` in seconds, at most 86400 (default 600; Claude and pi UserPromptSubmit, PreModelSwitch, and PostModelSwitch default 30; Claude and pi SessionEnd default 1.5, from 1.5 to 60; Codex SessionEnd and Interrupt default 1, at most 3). Claude and pi `async` command hooks have no timeout. On timeout or when the hook is cancelled, the process group gets SIGTERM, then SIGKILL after 5 seconds; on Windows, `taskkill /T /F` ends the process tree. Stdout is capped at 1 MB and stderr at 64 KB. Env: the extension env plus `CLAUDE_PROJECT_DIR`, the plugin vars, and `CLAUDE_ENV_FILE` for SessionStart. Codex `commandWindows` is ignored; `shell: "powershell"` is skipped with a log line.
- `async: true`: runs in the background (8 at a time) and cannot block. Its `additionalContext` is held for the next `before_agent_start`, and `systemMessage` shows a warning. `asyncRewake: true`: same, and exit 2 sends stderr (or stdout when stderr is empty) to the model as a `hooks-rewake` follow-up message that starts a turn. Codex SessionEnd hooks always run in the foreground.
- `onFailure: "block"` (Claude and pi): a start failure, an exit other than 0 or 2, a timeout, an HTTP error, or JSON that does not parse blocks on events where exit 2 blocks, except Stop and SubagentStop. On PermissionRequest it denies. The reason is `<Event>:<Tool> hook error: [<command>]: failed; blocking because onFailure is "block"` (`timed out` after a timeout), then stderr.
- `http` (Claude and pi): POSTs the event JSON to `url` with `headers`. `$VAR` and `${VAR}` in headers expand only for names in `allowedEnvVars`; other names become empty. The trust prompt shows the header templates and the `allowedEnvVars` names. A 2xx body that is empty or a JSON object is read like stdout; another 2xx body and other statuses are non-blocking errors. Default timeout 600 seconds.
- `prompt` (Claude and pi): asks the session model (or `model`) through `llm.complete` with the prompt, `$ARGUMENTS` replaced by the event JSON (appended when there is no placeholder). The reply must hold `{ "ok": bool, "reason"? }` or `{ "decision", "reason" }`; On `ok: false`: Stop and SubagentStop continue the turn with the reason, unless the reply sets `impossible: true`; PreToolUse and PostToolUse end the turn (block plus `continue: false`), or with `continueOnBlock: true` give the reason to the model; PermissionRequest and PermissionDenied ignore it; other events block. Default timeout 30 seconds.
- `once: true` (pi-native hooks only; Claude and Codex settings ignore it): after one run that exits 0 and does not block, the hook does not run again this session.
- The same handler in more than one file runs once per event. `statusMessage`: shown as the `hooks` status while the handlers run.

### CLAUDE_ENV_FILE

SessionStart hooks get `CLAUDE_ENV_FILE=<agentDir>/ext/hooks/env/<session_id>.sh`. When that file is not empty, each later `bash` tool call runs `. '<file>'; <command>` (after hook updates to the input).

## Paths

| Path | Content |
| --- | --- |
| `<agentDir>/ext/hooks.json`, `<cwd>/.pi/ext/hooks.json` | Options, `disabled` lists, and pi-native hooks. |
| `<agentDir>/ext/hooks/state.json` | `{ trusted, denied, disabled }` lists of hook hashes. `/hooks` reads `disabled` here and writes new disabled hooks to the options files. |
| `<agentDir>/ext/hooks/last-runs.json` | Last run per hook hash: time, duration, exit code, result, stdout, stderr (4,000 characters each). |
| `<agentDir>/ext/hooks/env/<session_id>.sh` | `CLAUDE_ENV_FILE`. |
| `<tmp>/hook_outputs/<session_id>/` | Hook output over the size limits. |

## Differences from upstream and unsupported items

Events marked pi-rust come from host events that only pi-rust sends; with an older pi they do not fire. Setup, the worktree flag and command, `/add-dir`, the task events, the watcher, and the subagent-based events work with any pi.

- PermissionRequest runs on each tool call that PreToolUse did not allow or deny, because pi has no permission dialog.
- Handler types `agent` and `mcp_tool` (Claude and Codex) are skipped with a log line, because the extension has no call path into pi's MCP client or a subagent runner. Codex `prompt` and `agent` are skipped, as in Codex.
- Codex matcher regexes use the Rust `regex` syntax, as Codex does: lookaround and backreferences are invalid there and match nothing.
- The per-event handler type limits of Claude (for example, SessionStart allows only `command` and `mcp_tool`) are not checked, and prompt hooks have no limit of 12 at a time.
- SessionEnd hooks each get their own timeout, not one shared budget. `CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS` is not read.
- On Windows, shell-form hooks run with `cmd`, not Git Bash or PowerShell.
- Codex: `trusted_hash` in `[hooks.state]`, `project_root_markers`, plugin hooks (`.codex-plugin/plugin.json` and plugin `hooks/hooks.json`), and `continue: false` on SessionStart `compact` and PostCompact are not supported.
- Input fields that are not sent: `scratchpad_dir` (pi has no scratchpad), `agent_id`/`agent_type` in the main session, and Codex `agent_transcript_path`. `permission_mode` is always `default`, because pi has no permission modes.
- Output fields: UserPromptSubmit `sessionTitle` sets the session name. A PermissionRequest deny with `decision.interrupt: true` aborts the run. `<system-reminder>` tags in context reach the model with `<` as `&lt;`. PostToolUse `classifierContext` and UserPromptSubmit `suppressOriginalPrompt` have no effect, because pi has no auto mode classifier and shows no block message with the prompt text.
- Unsupported handler and settings fields: `shell: "powershell"` on non-Windows, `commandWindows`/`command_windows`, Claude `updatedPermissions`, `updatedMCPToolOutput` beyond replacing the content, SessionStart `reloadSkills`, managed settings (`allowManagedHooksOnly`, `allowedHttpHookUrls`, `httpHookAllowedEnvVars`, Codex `requirements.toml`), plugin `hooks/hooks.json` files outside the pi-native file, and skill or subagent frontmatter hooks.
- `permission_mode` is `default`. `prompt_id`, `scratchpad_dir`, `effort`, and `agent_id`/`agent_type` in the main session are not sent.
- SubagentStart context and SubagentStop blocks go to `pi.events` channels, because the extension cannot reach the subagent session.
- A denied hook stays denied with `trustAll`.
