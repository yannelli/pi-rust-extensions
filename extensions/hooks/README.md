# hooks

Created 2026-10-29. Last updated 2026-10-30.

Runs Claude Code and Codex CLI hooks from pi. It reads the hook configs of both tools and a pi-native hooks file, maps pi events onto hook events, runs the matching handlers, and applies their decisions to pi. The contract it follows is `research/hooks.md` (Claude Code and Codex docs read 2026-10-29).

## Commands and flags

| Name | Use |
| --- | --- |
| `/hooks` | Opens the hooks manager panel. Without a UI it shows the listing as a notification. |
| `/hooks list` | Shows the listing as a notification. |
| `--hooks-trust-all` | Runs every hook without a trust prompt for this run (like Codex `--dangerously-bypass-hook-trust`). |

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
| Codex | `$CODEX_HOME` (else `~/.codex`) `hooks.json` and `config.toml` | `<root>/.codex/hooks.json`, `<root>/.codex/config.toml` |
| pi | `<agentDir>/ext/hooks.json` | `<cwd>/.pi/ext/hooks.json` |

- `<root>` is the session cwd when it holds `.claude` (or `.codex`), else the nearest parent directory with `.git`, else the cwd. `CLAUDE_PROJECT_DIR` is the Claude `<root>`.
- Hooks from all files merge.
- Project files of all three sources load only when the ctx snapshot says `isProjectTrusted: true`. Otherwise the extension skips them and shows one warning per session that lists the skipped files. Options in an untrusted project `.pi/ext/hooks.json` are skipped too.
- `disableAllHooks: true` in any loaded Claude file turns off the Claude source.
- Codex `[features] hooks = false` (or `codex_hooks = false`) turns off the Codex source. A project `config.toml` value overrides the global one.
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
| `session_start` | SessionStart (`startup`, `resume` for reload and resume, `clear` for new, `fork`) | Reloads config. `additionalContext` and plain stdout are held for the next `before_agent_start`. `sessionTitle` sets the session name. `initialUserMessage` is sent as a user message. |
| `session_shutdown` | SessionEnd (`prompt_input_exit` for quit, `clear`, `resume`, else `other`; Codex `other`) | Advisory. |
| `input` (interactive and rpc) | UserPromptSubmit | Block (exit 2, `decision: "block"`, `continue: false`) answers `handled` and shows the reason. Context transforms the text to `text + "\n\n" + context`. Resets `stop_hook_active`. |
| `before_agent_start` | none | Delivers held context once as `{ message: { customType: "hooks-context", content, display: false } }`. |
| `tool_call` | PreToolUse, then PermissionRequest (unless PreToolUse allowed or denied) | Deny gives `{ block: true, reason }`. Precedence: deny > defer > ask > allow. `ask` asks with `ui.confirm`; without a UI the call is blocked. `updatedInput` replaces the whole input (the patch removes keys the hook dropped). `additionalContext` is appended to the tool result. `continue: false` blocks with `stopReason`. |
| `tool_result` | PostToolUse, or PostToolUseFailure when `isError` (Claude and pi; Codex gets PostToolUse) | Claude `decision: "block"` appends the reason; Codex replaces the content with it. Exit 2 stderr and `additionalContext` are appended. Claude `updatedToolOutput` replaces the content (a string becomes a text block, other values become JSON text). `continue: false` appends `stopReason` and aborts the run. |
| `turn_end` with tool results | PostToolBatch (Claude and pi) | A block reason is sent as a `hooks-feedback` steer message. |
| `agent_end` | none | Keeps the last assistant text and whether it ended with an error. |
| `agent_settled` | Stop; StopFailure (Claude and pi) after an error; Codex Interrupt when aborted | A Stop block sends `hooks-stop` with the reason (`display: false`, `deliverAs: followUp`, `triggerTurn: true`) and sets `stop_hook_active` until the next user input. `continue: false` from any hook wins. |
| `session_before_compact` | PreCompact (`manual`, else `auto`) | Block or `continue: false` cancels the compaction. |
| `session_compact` | PostCompact, then SessionStart with source `compact` | Context is held. |
| `model_select` | PreModelSwitch, then PostModelSwitch (Claude and pi) | pi sends `model_select` after the switch, so a block switches back to the previous model and shows the reason. PostModelSwitch context is held. |
| `pi.events` subagent channels | SubagentStart, SubagentStop | See below. |

Matcher targets: tool name for tool events, `source` for SessionStart, `reason` for SessionEnd, `trigger` for Pre/PostCompact, agent type for Subagent events, error type for StopFailure (`rate_limit`, `authentication_failed`, `billing_error`, `max_output_tokens`, `server_error`, `invalid_request`, `unknown`, from the error text), model id for Pre/PostModelSwitch. UserPromptSubmit, PostToolBatch, Stop, and Interrupt ignore the matcher.

### Subagent channel contract

The extension subscribes to `subagents:start` and `subagent:async-started` (SubagentStart), and `subagents:stop`, `subagent:async-complete`, `subagent:foreground-complete` (SubagentStop). Data: `{ agentId, agentType, sessionId?, lastAssistantMessage? }`; `id`/`runId`, `agent`/`type`, and `result`/`output` are read as fallbacks. SubagentStart context is emitted on `hooks:subagent-context` `{ agentId, additionalContext }`. A SubagentStop block is emitted on `hooks:subagent-stop-blocked` `{ agentId, reason }`.

## Matchers

- Claude and pi: `*`, `""`, or omitted match all. A matcher of only `[A-Za-z0-9_\- ,|]` is an exact name or a `|`/`,` list. Other matchers are unanchored regexes (`Edit.*` also matches `NotebookEdit`).
- Codex: unanchored regex. For tools, the regex is tested against the Claude name and the Codex aliases: `Bash` also as `shell`, `exec_command`, `local_shell`; `Edit` and `Write` also as `apply_patch`.
- An invalid regex matches nothing and is logged.
- `if` (Claude, tool events): one rule such as `Bash(git *)` or `Edit(*.ts)`. The tool name must match, and the pattern (`*` is a wildcard, a trailing `:*` is a prefix) must match the whole primary input: Bash `command`, Read/Edit/Write/LS `file_path` or `path`, Glob/Grep `pattern`.

Tool names: `bash` -> `Bash`, `read` -> `Read`, `edit` -> `Edit`, `write` -> `Write`, `grep` -> `Grep`, `find` -> `Glob`, `ls` -> `LS`, `powershell` -> `PowerShell`; other names (MCP tools too) are unchanged. `tool_input` adds `file_path` next to `path` for read/edit/write, and `old_string`/`new_string` for edit (top level for one edit, and in each `edits` item). `updatedInput` maps back to pi names; `file_path` wins over `path`.

## Stdin

All events: `session_id`, `transcript_path` (session file or `null`), `cwd`, `hook_event_name`, `permission_mode: "default"`. Codex also gets `model` (`provider/id`) and, on UserPromptSubmit, PreToolUse, PermissionRequest, PostToolUse, and Stop, `turn_id`. Tool events: `tool_name`, `tool_input`, `tool_use_id`; PostToolUse adds `tool_response` `{ content, text, isError, details }`, PostToolUseFailure adds `error`. PostToolBatch: `tool_results`. Stop: `stop_hook_active`, `last_assistant_message`; StopFailure adds `error` and `error_details`. UserPromptSubmit: `prompt`. SessionStart: `source`, `model`. SessionEnd: `reason`. Pre/PostCompact: `trigger`; PreCompact `custom_instructions`; PostCompact `compact_summary`. Pre/PostModelSwitch: `model`, `provider`, `previous_model`, `previous_provider`, `source`. Subagent events: `agent_id`, `agent_type`; SubagentStop adds `stop_hook_active` and `last_assistant_message`.

## Output rules

- Stdout counts as JSON only when the trimmed text starts with `{`, ends with `}`, and parses.
- Exit 0: JSON is applied. Exit 2: blocks on PreToolUse, UserPromptSubmit, Stop, SubagentStop, PostToolBatch, PreCompact, PreModelSwitch (Codex: PreToolUse, UserPromptSubmit, Stop, SubagentStop), with stderr as the reason; Claude JSON is still read but cannot override the block. On PostToolUse, exit 2 stderr goes to the model. Other exit codes are non-blocking errors; Claude JSON still applies.
- PreToolUse exit 2 reason (Claude and pi): `PreToolUse:<Tool> hook error: [<command>]: <stderr>`.
- `permissionDecision` is read under `hookSpecificOutput` and at the top level. Legacy `decision: approve|block` maps to allow and deny. Codex `ask`, `continue`, and `stopReason` on PreToolUse mark the hook failed and the tool runs; Codex `updatedInput` needs `allow`.
- Plain stdout is context only for UserPromptSubmit, SessionStart, PostModelSwitch (Claude and pi) and SessionStart, UserPromptSubmit, SubagentStart (Codex). Codex Stop and SubagentStop need JSON.
- Universal JSON: `continue: false` stops and wins over decisions; `systemMessage` shows a warning; `suppressOutput` has no effect; `terminalSequence` is written to the terminal when it is BEL or OSC 0/1/2/9/99/777.
- Codex `additionalContextLimit` (tokens, default 2500, about 4 characters per token, 0 for no limit): longer context goes to `<tmp>/hook_outputs/<session_id>/<n>.txt` and the model gets the path and a 2,000-character preview.

## Handlers

- `command`: `sh -c <command>` (`cmd /d /s /c` on Windows), or exec form without a shell when `args` is set. Stdin is the event JSON; the cwd is the session cwd. `timeout` in seconds, at most 86400 (default 600; Codex SessionEnd and Interrupt default 1, at most 3). On timeout or when the hook is cancelled, the process group gets SIGTERM, then SIGKILL after 5 seconds; on Windows, `taskkill /T /F` ends the process tree. Stdout is capped at 1 MB and stderr at 64 KB. Env: the extension env plus `CLAUDE_PROJECT_DIR`, the plugin vars, and `CLAUDE_ENV_FILE` for SessionStart. Codex `commandWindows` is ignored; `shell: "powershell"` is skipped with a log line.
- `async: true`: runs in the background (8 at a time), cannot block, and only records its result. `asyncRewake: true`: same, and exit 2 sends stderr to the model as a `hooks-rewake` follow-up message that starts a turn.
- `onFailure: "block"` (Claude and pi): a nonzero exit other than 2, or a timeout, blocks on events where exit 2 blocks.
- `http` (Claude and pi): POSTs the event JSON to `url` with `headers`. `$VAR` and `${VAR}` in headers expand only for names in `allowedEnvVars`; other names become empty. The trust prompt shows the header templates and the `allowedEnvVars` names. A 2xx body is read like stdout; other statuses are non-blocking errors. Default timeout 600 seconds.
- `prompt` (Claude and pi): asks the session model (or `model`) through `llm.complete` with the prompt, `$ARGUMENTS` replaced by the event JSON (appended when there is no placeholder). The reply must hold `{ "ok": bool, "reason"? }` or `{ "decision", "reason" }`; `ok: false` blocks with the reason. Default timeout 30 seconds.
- `once: true`: runs once per session. `statusMessage`: shown as the `hooks` status while the handlers run.

### CLAUDE_ENV_FILE

SessionStart hooks get `CLAUDE_ENV_FILE=<agentDir>/ext/hooks/env/<session_id>.sh`. When that file is not empty, each later `bash` tool call runs `. '<file>'; <command>` (after hook updates to the input).

## Paths

| Path | Content |
| --- | --- |
| `<agentDir>/ext/hooks.json`, `<cwd>/.pi/ext/hooks.json` | Options, `disabled` lists, and pi-native hooks. |
| `<agentDir>/ext/hooks/state.json` | `{ trusted, denied, disabled }` lists of hook hashes. `/hooks` reads `disabled` here and writes new disabled hooks to the options files. |
| `<agentDir>/ext/hooks/last-runs.json` | Last run per hook hash: time, duration, exit code, result, stdout, stderr (4,000 characters each). |
| `<agentDir>/ext/hooks/env/<session_id>.sh` | `CLAUDE_ENV_FILE`. |
| `<tmp>/hook_outputs/<session_id>/` | Codex context overflow. |

## Differences from upstream and unsupported items

Claude Code and Codex events that do not fire, because pi has no matching event:

| Event | Source | Reason |
| --- | --- | --- |
| Setup | Claude | pi has no setup or install phase. |
| InstructionsLoaded | Claude | pi sends no event when it loads AGENTS.md or rules files. |
| UserPromptExpansion | Claude | pi sends no event for prompt template expansion. |
| MessageDisplay | Claude | Extensions get no hook point before pi renders a message. |
| PermissionDenied | Claude | pi has no permission dialog, so it denies nothing. |
| Notification | Claude | pi sends no event for its own notifications. |
| TaskCreated, TaskCompleted | Claude | pi has no task model. |
| TeammateIdle | Claude | pi has no agent teams. |
| ConfigChange | Claude | pi sends no event when settings files change. |
| CwdChanged, DirectoryAdded | Claude | The pi session cwd is fixed, and pi has no added directories. |
| FileChanged | Claude | pi does not watch files. |
| WorktreeCreate, WorktreeRemove | Claude | pi has no worktree commands. |
| Elicitation, ElicitationResult | Claude | pi sends no MCP elicitation events to extensions. |

- PermissionRequest runs on each tool call that PreToolUse did not allow or deny, because pi has no permission dialog.
- Handler types `agent` and `mcp_tool` are skipped with a log line. Codex `prompt` and `agent` are skipped, as in Codex.
- Unsupported handler and settings fields: `shell: "powershell"` on non-Windows, `commandWindows`/`command_windows`, Claude `updatedPermissions`, `updatedMCPToolOutput` beyond replacing the content, SessionStart `watchPaths` and `reloadSkills`, managed settings (`allowManagedHooksOnly`, `allowedHttpHookUrls`, `httpHookAllowedEnvVars`, Codex `requirements.toml`), plugin `hooks/hooks.json` files outside the pi-native file, and skill or subagent frontmatter hooks.
- `permission_mode` is `default`. `prompt_id`, `scratchpad_dir`, `effort`, and `agent_id`/`agent_type` in the main session are not sent.
- SubagentStart context and SubagentStop blocks go to `pi.events` channels, because the extension cannot reach the subagent session.
- A denied hook stays denied with `trustAll`.
