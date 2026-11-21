# hooks

Created 2026-10-29. Last updated 2026-11-21.

Runs Claude Code and Codex CLI hooks from pi. It reads the hook configs of both tools and a pi-native hooks file, maps pi events onto hook events, runs the matching handlers, and applies their decisions to pi. It follows these docs, read 2026-11-20: the Claude Code hooks reference (code.claude.com/docs/en/hooks), settings reference (code.claude.com/docs/en/settings-reference), and managed settings (code.claude.com/docs/en/managed-settings), and the Codex hooks docs (developers.openai.com/codex/hooks) with the Codex 0.171.0 hook schemas.

## Commands and flags

| Name | Use |
| --- | --- |
| `/hooks` | Opens the hooks manager panel. Without a UI it shows the listing as a notification. |
| `/hooks list` | Shows the listing as a notification. Its first line shows the sources, the load scope, `trustAll`, and `conflicts`. |
| `--hooks-trust-all` | Runs every hook without a trust prompt for this run (like Codex `--dangerously-bypass-hook-trust`). |
| `--init`, `--init-only`, `--maintenance` | Run Setup hooks, as the Claude flags do. `--init` and `--maintenance` work in print mode (no UI). `--init-only` runs Setup and SessionStart hooks, then shuts pi down. |
| `--worktree <name>` | Starts in a new worktree: WorktreeCreate hooks, else `git worktree add -b <name> <git root>/.pi/worktrees/<name>`. Needs a pi host that lets `session_start` handlers switch sessions. |
| `/worktree [name]` | Creates a worktree the same way and switches to a new session in it. `/worktree exit [--force]` switches back and removes it (WorktreeRemove hooks, else `git worktree remove`). On quit, a clean worktree from `/worktree` is removed. |
| `/add-dir <path>` | Adds a working directory: tells the model, then runs DirectoryAdded hooks (`source: "slash_command"`) in the background. Their `systemMessage` goes to the model on the next turn. |

### Hooks manager

`/hooks` opens a panel laid out like `/deferred` and `/claude-plugins`. The panel stays open while you work: each change saves at once and shows its result in the status line.

- Tabs (Tab and Shift+Tab): **All**, **Project**, **Global**, and **Settings**, each with a count.
- The hook tabs list each hook handler under its source: Claude Code (managed settings, user `settings.json`, project `.claude/settings.json`, local `.claude/settings.local.json`), Codex (`hooks.json`, inline `config.toml` hooks, and `requirements.toml`), pi (`ext/hooks.json` files), and Plugins (pi-native groups with a `plugin` id).
- Each row shows the enabled state (`●` on, `○` off), event, matcher, layer (`managed`, `global`, `project`, or `local`), trust state, and the command. Below 72 columns the layer and trust state shrink to two characters, such as `G✓` (global, trusted), `L?` (local, untrusted), or `MM` (managed).
- A `[conflict]` tag marks a hook whose event and matcher another layer also defines with a different handler. Under `override`, a `[shadowed]` tag marks a hook that a more specific layer replaces. The actions pane names the other layers.
- The Settings tab holds the options of the global options file: the Claude Code, Codex, and pi sources (`on` or `off`), where hooks load from (`global`, `project`, or `both`), `trustAll`, and `conflicts` (`merge`, `warn`, or `override`, see [Conflicts between layers](#conflicts-between-layers)). The title line shows a `conflicts` mode other than `merge` and `trustAll on`.

| Key | Action |
| --- | --- |
| Up/Down, Home/End, PageUp/PageDown | Move. Up and Down wrap. |
| Left/Right | On a hook: turn it on or off. On a setting: step to the previous or next value. |
| Enter | On a hook: the actions pane with its details (full command, type, timeout, `async`/`asyncRewake`/`once` flags, status message, file, plugin, trust, conflict, and the last run with stdout and stderr) and these actions: Enable or Disable, Trust always, Allow once this session, Untrust, Deny, and Remove from file. Remove asks for a second Enter. On a setting: its values, with the current one marked. On **Add a hook…**: the add form. |
| Typing | Searches the current tab (event, matcher, command, source, layer, file, plugin id, and tag). Backspace deletes. |
| Esc | Clears the search, then closes. In the actions pane or the add form, Esc goes back. |

**Add a hook…** is the first row of the hook tabs. Its form adds a command hook in the panel: pick the event with Up and Down, then type the matcher (empty matches all) and the command, with Enter after each. Tab picks the scope (`global`, or `project` in a trusted project). Backspace on an empty field goes back one step.

Writes per scope:

- Disable and enable add or remove the hook hash in the `disabled` list of the pi options file of the hook's scope: `<agentDir>/ext/hooks.json` for global hooks, `<cwd>/.pi/ext/hooks.json` for project and local hooks. The hook definition stays in its file. Enable also clears the hash from the older `state.json` `disabled` list.
- Remove deletes the handler from its own file (Claude settings, Codex `hooks.json` or `config.toml`, or the pi file), then drops a matcher group or event that became empty. JSON files keep their key order, unknown keys, indent, and trailing newline; `config.toml` keeps comments and formatting (`toml_edit`). A file that does not parse is not changed. Remove also clears the hash from the trust and disabled lists.
- Add appends `{ matcher?, hooks: [{ type: "command", command }] }` to the pi file of the chosen scope and marks the new hook trusted.
- Settings write their key to the global options file.
- Project scope needs a trusted project (`isProjectTrusted`). In an untrusted project the panel shows a warning, hides project hooks, and offers only the global scope for a new hook.
- Managed hooks (organization policy files) cannot be disabled, removed, or untrusted from the panel.

Each change reloads the config, so the running session uses it at once.

The extension registers no tools.

## Sources

| Source | Managed | Global | Project |
| --- | --- | --- | --- |
| Claude | `managed-settings.json`, then `managed-settings.d/*.json` in name order (hidden files skipped), in `/Library/Application Support/ClaudeCode` (macOS), `/etc/claude-code` (Linux), or `C:\Program Files\ClaudeCode` (Windows) | `$CLAUDE_CONFIG_DIR/settings.json`, else `~/.claude/settings.json` | `<root>/.claude/settings.json`, `<root>/.claude/settings.local.json` (layer `local`) |
| Codex | System dir `/etc/codex` (Unix) or `%ProgramData%\OpenAI\Codex` (Windows, `C:\ProgramData` when the variable is unset): `hooks.json`, `config.toml`, and `[hooks]` of `requirements.toml` | `$CODEX_HOME` (else `~/.codex`): `hooks.json` and `config.toml` | Each `.codex/hooks.json` and `.codex/config.toml` from the project root down to the cwd |
| pi | none | `<agentDir>/ext/hooks.json` | `<cwd>/.pi/ext/hooks.json` |

- `<root>` is the session cwd when it holds `.claude` (or `.codex`), else the nearest parent directory with `.git`, else the cwd. `CLAUDE_PROJECT_DIR` is the Claude `<root>`.
- In a git worktree without one of the project files, the main checkout's copy applies. This covers the untracked `.claude/settings.local.json`.
- Hooks from all files merge; the `conflicts` option decides what runs when several layers define the same event and matcher (see [Conflicts between layers](#conflicts-between-layers)).
- Managed hooks load in every `scope` option and project trust state while their source (`sources.claude` or `sources.codex`) is on. They are trusted by policy, ignore `disabled` lists and Codex `[hooks.state]`, and are not shadowed.
- The Codex project root is the nearest dir with one of the user `config.toml` `project_root_markers` (default `[".git"]`); `project_root_markers = []` makes the cwd the root.
- Project files of all three sources load only when the ctx snapshot says `isProjectTrusted: true`. Otherwise the extension skips them and shows one warning per session that lists the skipped files. Options in an untrusted project `.pi/ext/hooks.json` are skipped too.
- `disableAllHooks` follows Claude settings precedence: a managed value wins, else the value in the most specific loaded file (local, then project, then user). A project `false` overrides a user `true`. A managed `true` turns off all Claude hooks. A `true` outside managed settings turns off user, project, and local hooks; managed hooks keep running. Managed `allowManagedHooksOnly: true` keeps only managed Claude hooks. In both cases pi-native groups with a `plugin` id are off too, because they come from Claude Code plugins.
- Claude `allowedHttpHookUrls` and `httpHookAllowedEnvVars` merge across the managed, user, project, and local files. When any file sets `allowedHttpHookUrls`, a Claude or pi http hook runs only when its URL matches a pattern (`*` is a wildcard; the scheme and host compare without case and without a trailing dot). When any file sets `httpHookAllowedEnvVars`, a header can use a variable only when both that list and the hook's `allowedEnvVars` name it.
- Codex `[features] hooks = false` (or `codex_hooks = false`) turns off the Codex source. A `[features] hooks` value in `requirements.toml` wins; else the most specific layer that sets it wins. `requirements.toml` `allow_managed_hooks_only = true` keeps only the system and `requirements.toml` hooks.
- Codex `[hooks.state."<file>:<snake_event>:<group>:<handler>"] enabled = false` in the user `config.toml` turns off one Codex hook, as Codex `/hooks` does. `trusted_hash` is not read; pi uses its own trust list.
- Codex inline hooks in `config.toml` (`[[hooks.<Event>]]` with `[[hooks.<Event>.hooks]]`) merge with `hooks.json` of the same layer, with a log warning.
- pi-native files use the Claude settings shape and Claude semantics. Matcher groups can carry `plugin`, `pluginRoot`, `pluginData`, and `pluginEnv` (written by `claude-marketplace`). The hook gets `CLAUDE_PLUGIN_ROOT`, `PLUGIN_ROOT`, `CLAUDE_PLUGIN_DATA`, and `PLUGIN_DATA` from them, and the string values of `pluginEnv` whose names start with `CLAUDE_PLUGIN_OPTION_` (other names are ignored). `/hooks` shows the plugin id.

## Options

In `<agentDir>/ext/hooks.json` and `<cwd>/.pi/ext/hooks.json` (same file as the pi-native hooks). The project file wins per key; `sources` merges per sub-key. `/hooks` writes these keys and keeps all other keys.

| Key | Default | Use |
| --- | --- | --- |
| `sources.claude`, `sources.codex`, `sources.pi` | `true` | Turn a source on or off. |
| `scope` | `"both"` | `global`, `project`, or `both`: which files load. |
| `trustAll` | `false` | Run all hooks without trust prompts. Read from the global options file only; a project file's `trustAll` is ignored. |
| `conflicts` | `"merge"` | `merge`, `warn`, or `override`; see [Conflicts between layers](#conflicts-between-layers). Read from the global options file only, because a project options file can come from the repository. |
| `disabled` | `[]` | Hashes of disabled hooks. The lists of both files merge. |
| `limits/promptAgentHooks` | `false` | Claude Code limit on prompt and agent hooks; see [Handlers](#handlers). Read from the global options file only. |
| Feature switches | per switch | On and off rows that features add to the Settings tab, each under its own key (for example `managed/<name>`). Read from the global options file only. |

## Conflicts between layers

Each hook has a layer. The layers, from the least specific to the most specific, are:

1. `global`: Claude user settings, Codex `$CODEX_HOME`, and the pi global file.
2. `project`: Claude `.claude/settings.json`, each Codex `.codex` dir, and the pi project file. A deeper Codex dir is more specific.
3. `local`: `.claude/settings.local.json`.

Managed hooks are not in this order. A conflict occurs when one family has different handlers for the same event and the same matcher in more than one layer. The matchers `*`, `""`, and an omitted matcher are equal. Claude hooks and pi hooks are one family. Codex hooks are a second family, because they get a different stdin. Thus, a Claude hook does not conflict with a Codex hook. Two matchers that only overlap, such as `Bash` and `.*`, do not conflict.

| `conflicts` | Effect |
| --- | --- |
| `merge` (default) | The hooks of all layers run. Claude Code and Codex do the same. |
| `warn` | The hooks of all layers run. At load, one warning for each session shows the new conflicts. It also shows each command that Claude or pi config and Codex config both register for one event. A tool call shows a warning when its hooks give different decisions or when more than one hook changes the input. |
| `override` | For each conflict, only the hooks of the most specific layer run. If none of these hooks can run, the next layer runs. A hook cannot run when it is disabled, unsupported, or denied, or when it is untrusted and the user does not allow it at the prompt. Managed hooks always run. |

In all modes:

- The extension folds decisions in this order: deny, ask, allow. A deny from any layer blocks the call.
- A handler that is in more than one file runs one time for each event. The copy from the most specific layer that can run is the copy that runs. The copy of a plugin is separate.
- The extension reads the outcomes from the least specific layer to the most specific layer. Thus, where the last hook wins (for example, `updatedInput`), the most specific hook wins.

## Trust

Each hook has a hash: sha256 hex of the canonical JSON (sorted keys) `{ file, event, matcher, handler }`, with `file` the absolute config path and `matcher` `null` when omitted. A changed handler or a moved file gives a new hash.

- Managed hooks are trusted by policy.
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
| `tool_call` | PreToolUse, then PermissionRequest (unless PreToolUse allowed or denied) | Deny gives `{ block: true, reason }`. Precedence: deny > ask > allow; `defer` is ignored with a log line, as in an interactive Claude Code session. `ask` asks with `ui.confirm`; without a UI the call is blocked. `updatedInput` replaces the whole input (the patch removes keys the hook dropped). `additionalContext` is appended to the tool result. Claude `continue: false` does not block the call: the tool runs, then `stopReason` is appended to its result and the run aborts (a deny still blocks). Codex `allow` with `updatedInput` rewrites the input but does not approve; Codex `updatedInput` for `apply_patch` (pi edit and write) is ignored with a log line. |
| `tool_result` | PostToolUse, or PostToolUseFailure when `isError` (Claude and pi; Codex gets PostToolUse) | Claude `decision: "block"` appends the reason; Codex replaces the content with it. Exit 2 stderr and `additionalContext` are appended. Claude `updatedToolOutput` on PostToolUse replaces the content (a string becomes a text block, other values become JSON text); PostToolUseFailure cannot rewrite the result. Claude `continue: false` appends `stopReason` and aborts the run. Codex `continue: false` and Codex exit 2 replace the content with the reason, and the run continues. |
| `turn_end` with tool results | PostToolBatch (Claude and pi) | `additionalContext` is sent as a `hooks-context` steer message (`display: false`) before the next model call. A block or `continue: false` shows the reason, keeps it as a `hooks-feedback` message for the next turn, and aborts the run. |
| `agent_end` | none | Keeps the last assistant text and whether it ended with an error. |
| `agent_settled` | Stop; StopFailure (Claude and pi) after an error; Codex Interrupt when aborted | A Stop block sends `hooks-stop` with the reason (`display: false`, `deliverAs: followUp`, `triggerTurn: true`) and sets `stop_hook_active` until the next user input. Claude and pi `additionalContext` continue the turn in the same way. After 8 consecutive continuations (`CLAUDE_CODE_STOP_HOOK_BLOCK_CAP`), the next one is skipped and the turn ends; a tool call or user input resets the count. `continue: false` from any hook wins. |
| `session_before_compact` | PreCompact (`manual`, else `auto`) | Block or `continue: false` cancels the compaction. |
| `session_compact` | PostCompact, then SessionStart with source `compact` | Context is held. |
| `model_select` | PreModelSwitch, then PostModelSwitch (Claude and pi) | pi sends `model_select` after the switch, so a block switches back to the previous model and shows the reason. Exit 2, `decision: "block"`, `permissionDecision: "deny"`, `continue: false`, and a timeout block. `ask` asks with `ui.confirm`; without a UI it blocks. PreModelSwitch does not run for a `restore` switch. PostModelSwitch context is held. |
| `before_provider_request`, `after_provider_response` | Codex PreModelRequest, PostModelRequest | Diagnostic only; the request does not wait. Both get `request_id` (`req-<turn>-<n>`), `attempt_number: 1`, `thread_id` (the session id), and `kind: "sampling"`. PostModelRequest `status` is `completed` for a 2xx HTTP status, else `failed` with `error: "HTTP <status>"`; `token_usage` and `response_id` are `null`. |
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

Matcher targets: tool name for tool events, `source` for SessionStart, `reason` for SessionEnd, `trigger` for Pre/PostCompact, agent type for Subagent events, error type for StopFailure (`rate_limit`, `authentication_failed`, `oauth_org_not_allowed`, `account_on_hold`, `billing_error`, `invalid_request`, `model_not_found`, `server_error` (529 overloaded too), `max_output_tokens`, `cloud_credential_error`, `unknown`, from the error text), model id for Pre/PostModelSwitch, `trigger` for Setup, `load_reason` for InstructionsLoaded, `command_name` for UserPromptExpansion, `notification_type` for Notification, `source` for ConfigChange and DirectoryAdded, the file name for FileChanged, server name for Elicitation and ElicitationResult. UserPromptSubmit, PostToolBatch, Stop, Interrupt, TaskCreated, TaskCompleted, TeammateIdle, CwdChanged, MessageDisplay, PreModelRequest, and PostModelRequest ignore the matcher.

### Subagent channel contract

The extension subscribes to `subagents:start` and `subagent:async-started` (SubagentStart), and `subagents:stop`, `subagent:async-complete`, `subagent:foreground-complete` (SubagentStop). Data: `{ agentId, agentType, sessionId?, lastAssistantMessage? }`; `id`/`runId`, `agent`/`type`, and `result`/`output` are read as fallbacks. SubagentStart context is emitted on `hooks:subagent-context` `{ agentId, additionalContext }`. A SubagentStop block is emitted on `hooks:subagent-stop-blocked` `{ agentId, reason }`.

## Matchers

- Claude and pi: `*`, `""`, or omitted match all. A matcher of only `[A-Za-z0-9_\- ,|]` is an exact name or a `|`/`,` list. Other matchers are unanchored JavaScript regular expressions (`regress`, ECMAScript syntax with lookaround and backreferences): `Edit.*` also matches `NotebookEdit`, `^(?!mcp__)` matches every tool that is not an MCP tool.
- Claude StopFailure and FileChanged: the exact-name set is only `[A-Za-z0-9_|]`, so a hyphen, space, or comma makes the matcher a regex.
- Codex: `*` or `""` match all. A matcher of only `[A-Za-z0-9_|]` is an exact name or a `|` list; other matchers are unanchored regexes. Tool matchers test the Codex name and its aliases: `apply_patch` (pi edit and write) also as `Edit` and `Write`.
- An invalid regex matches nothing and is logged.
- `if` (Claude, tool events only; a hook with `if` on another event never runs): one rule such as `Bash(git *)` or `Edit(*.ts)`. The tool name must match, and the pattern (`*` is a wildcard, a trailing `:*` is a prefix) must match the whole primary input: Bash `command`, Read/Edit/Write/LS `file_path` (absolute, or relative to the cwd), Glob/Grep `pattern`.
- Bash `if` rules test each subcommand: the command is split on `&&`, `||`, `;`, `|`, `&`, and newlines outside quotes, `$()` and backtick bodies are added, and leading `VAR=value` words are removed. A pattern with more than the command name (`Bash(git push *)`) runs the hook for any command with `$`, `$()`, or backticks. Unclosed quotes run the hook.

Tool names: `bash` -> `Bash`, `read` -> `Read`, `edit` -> `Edit`, `write` -> `Write`, `grep` -> `Grep`, `find` -> `Glob`, `ls` -> `LS`, `powershell` -> `PowerShell`; other names (MCP tools too) are unchanged. `tool_input` adds an absolute `file_path` (`~` and relative paths resolved against the cwd) next to `path` for read/edit/write, and `old_string`/`new_string` for edit (top level for one edit, and in each `edits` item). `updatedInput` maps back to pi names; `file_path` wins over `path`.

Codex tool input: pi edit and write reach Codex hooks as `tool_name: "apply_patch"` with `tool_input: { command }`, an `*** Begin Patch` text (`*** Add File` for write, `*** Update File` with one `@@` hunk per edit for edit). Codex PostToolUse `tool_response` is the result text.

## Stdin

All events: `session_id`, `transcript_path` (session file or `null`), `cwd`, `hook_event_name`, `permission_mode: "default"`. Claude and pi also get `prompt_id` (a new id for each user input, absent before the first), `effort: { level }` on tool events, Stop, and SubagentStop when a thinking level is set (`minimal` and `low` are `low`), `session_title` on SessionStart and UserPromptSubmit when the session has a name, and `mcp_server: { name, source }` on tool events of `mcp__<server>__<tool>` tools (`source` from `pi.getMcpServers()`, else `user`). Codex also gets `model` (the model id) and, on UserPromptSubmit, PreToolUse, PermissionRequest, PostToolUse, Stop, PreCompact, PostCompact, SubagentStart, SubagentStop, Interrupt, PreModelRequest, and PostModelRequest, `turn_id`. Tool events: `tool_name`, `tool_input`, `tool_use_id`; PostToolUse adds `tool_response` `{ content, text, isError, details }`, PostToolUseFailure adds `error` and `is_interrupt` (`true` for an aborted call). PostToolUse and PostToolUseFailure add `duration_ms`, measured from the end of `tool_call` to `tool_result`. PostToolBatch: `tool_calls`, each `{ tool_name, tool_input, tool_use_id, tool_response }` with the result text. Stop: `stop_hook_active`, `last_assistant_message`, `background_tasks` (`[{ id, type, status: "running", description }]` from the `compactor:active-work` channel; `id` is the publisher, `type` is `shell` for background-tasks, `subagent` for subagents, else the publisher id, and `description` is capped at 1,000 characters), `session_crons: []`; StopFailure adds `error` and `error_details`. UserPromptSubmit: `prompt`. SessionStart: `source`, `model`; on `resume` and `fork` with an assistant response, `seconds_since_last_response`, `context_tokens`, `prompt_cache_likely_expired` (300-second cache), `estimated_cache_write_usd`. SessionEnd: `reason`. Pre/PostCompact: `trigger`; PreCompact `custom_instructions`; PostCompact `compact_summary`. Pre/PostModelSwitch: `from_model`, `to_model`, `requested_model` (`null` on `resume`), `source` (`command` for a set, `picker` for a cycle, `resume` for a restore, else `sdk`). PreModelSwitch and PostModelSwitch also get `context_tokens`, `prompt_cache_warm` (a response in the last 300 seconds), `cache_ttl: "5m"`, `estimated_cache_write_usd`, and `pricing` (`catalog` from the model's `cost.cacheWrite`, else `default` at 3.75 USD per million tokens). PostModelRequest: `status`, `error`, `token_usage`. Subagent events: `agent_id`, `agent_type`; SubagentStop adds `stop_hook_active`, `last_assistant_message`, and `agent_transcript_path` (`sessionFile` or `transcriptPath` of the channel data).

## Output rules

- Stdout counts as JSON only when the trimmed text starts with `{`, ends with `}`, and parses.
- Exit 0: JSON is applied. Text that starts with `{` and ends with `}` but does not parse is a non-blocking error and is not used as context.
- Exit 2: blocks on PreToolUse, UserPromptSubmit, Stop, SubagentStop, PostToolBatch, PreCompact, PreModelSwitch (Codex: PreToolUse, PermissionRequest (deny), PostToolUse (replaces the result), UserPromptSubmit, Stop, SubagentStop). The reason is the reason of a blocking JSON decision, else stderr. Claude JSON is still read but cannot override the block. Claude and pi PostToolUse exit 2 stderr goes to the model. A Codex exit 2 with empty stderr is a non-blocking error. A Claude Stop, SubagentStop, TaskCompleted, or plugin UserPromptSubmit exit 2 with empty stdout and `No such file or directory` on stderr is a non-blocking error.
- Other exit codes are non-blocking errors; Claude JSON still applies.
- PreToolUse exit 2 reason (Claude and pi): `PreToolUse:<Tool> hook error: [<command>]: <stderr>`.
- Claude and pi `permissionDecision` is read under `hookSpecificOutput` and at the top level. Legacy `decision: approve|block` maps to allow and deny.
- Codex reads `permissionDecision` under `hookSpecificOutput` only. These outputs mark the run failed, and the action continues: on PreToolUse `ask`, `allow` without `updatedInput`, `updatedInput` without `allow`, `deny` without a reason, `decision: "approve"`, `continue: false`, `stopReason`, `suppressOutput`; on PermissionRequest `updatedInput`, `updatedPermissions`, `interrupt: true`, `continue: false`, `stopReason`, `suppressOutput`; on PostToolUse `updatedMCPToolOutput`, `suppressOutput`; `decision: "block"` without a reason on any event. Codex PreCompact reads only the universal fields.
- Plain stdout is context only for UserPromptSubmit, SessionStart, PostModelSwitch (Claude and pi) and SessionStart, UserPromptSubmit, SubagentStart (Codex). Codex Stop and SubagentStop need JSON.
- Universal JSON: `continue: false` stops and wins over decisions; `systemMessage` shows a warning; `suppressOutput` has no effect; `terminalSequence` is written to the terminal when it is BEL or OSC 0/1/2/9/99/777.
- Claude and pi: `additionalContext`, `systemMessage`, `initialUserMessage`, and plain stdout are capped at 10,000 characters each. Longer text goes to `<tmp>/hook_outputs/<session_id>/<id>.txt` and is replaced by the path and the first 2,000 characters.
- Codex `additionalContextLimit` (tokens, default 2500, about 4 characters per token, 0 for no limit) applies to `additionalContext` on PreToolUse, PostToolUse, SessionStart, UserPromptSubmit, and SubagentStart; on other events it is ignored with a log line. Feedback, block reasons, and `systemMessage` keep the 2,500-token default. Longer text goes to the same directory and is replaced by the path and a head and tail preview within the limit.

## Handlers

- `command`: `sh -c <command>` (Codex: `$SHELL -lc <command>`, else `/bin/sh -lc`), or exec form without a shell when `args` is set. In exec form, `${CLAUDE_PROJECT_DIR}`, `${CLAUDE_PLUGIN_ROOT}`, and `${CLAUDE_PLUGIN_DATA}` in `command` and `args` are replaced as plain strings. Stdin is the event JSON; the cwd is the session cwd. `timeout` in seconds, at most 86400 (default 600; Claude and pi UserPromptSubmit, PreModelSwitch, and PostModelSwitch default 30, MessageDisplay 10; Claude and pi SessionEnd default 1.5, from 1.5 to 60; Codex SessionEnd and Interrupt default 1, at most 3). A Claude or pi timeout under 1 second counts as 1 second. Claude and pi `async` command hooks have no timeout. On timeout or when the hook is cancelled, the process group gets SIGTERM, then SIGKILL after 5 seconds; on Windows, `taskkill /T /F` ends the process tree. Stdout is capped at 1 MB and stderr at 64 KB. Env: the extension env plus `CLAUDE_PROJECT_DIR`, the plugin vars, `CLAUDE_EFFORT` (Claude and pi, when a thinking level is set, the same level as stdin `effort`), and `CLAUDE_ENV_FILE` for SessionStart. On Windows, Codex `commandWindows` (TOML `command_windows`) replaces `command`. `shell: "powershell"` is skipped with a log line except on Windows. Plugin hooks in exec form replace `${user_config.<key>}` in `command` and `args` with `CLAUDE_PLUGIN_OPTION_<KEY>` from `pluginEnv`; a shell-form plugin hook whose `command` holds `${user_config.` is skipped with a log line, as Claude Code refuses it.
- On Windows, shell-form hooks run with Git Bash, as Claude Code does, so POSIX hooks behave the same on every platform: `bash -c <command>` (Codex: `bash -lc`, with `CHERE_INVOKING=1` to keep the cwd). bash is pi's `shellPath` setting, then `CLAUDE_CODE_GIT_BASH_PATH`, Git for Windows under `%ProgramFiles%`, `%ProgramFiles(x86)%`, or `%LOCALAPPDATA%\Programs`, `bin\bash.exe` next to `git.exe` on `PATH`, then `bash.exe` on `PATH` (the WSL launcher in `System32` and `WindowsApps` is skipped). `shell: "powershell"` runs `pwsh.exe` (else Windows PowerShell) with `-NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand`. Without bash or PowerShell the hook runs with `cmd /d /s /c`, and the extension logs a warning once.
- `async: true`: runs in the background (8 at a time) and cannot block. A Claude or pi async hook can print its JSON alone on stdout or on a line of its own among other text. Its `additionalContext` is held for the next `before_agent_start`, and `systemMessage` shows a warning. `asyncRewake: true`: same, and exit 2 sends stderr (or stdout when stderr is empty) to the model as a `hooks-rewake` follow-up message that starts a turn. Codex SessionEnd hooks always run in the foreground.
- `onFailure: "block"` (Claude and pi): a start failure, an exit other than 0 or 2, a timeout, an HTTP error, JSON that does not parse, or an `mcp_tool` answer that is not empty, `{}`, or JSON that sets a field other than `reason` blocks on events where exit 2 blocks, except Stop, SubagentStop, TaskCompleted, and TeammateIdle. On PermissionRequest it denies. The reason is `<Event>:<Tool> hook error: [<command>]: failed; blocking because onFailure is "block"` (`timed out` after a timeout), then stderr.
- `http` (Claude and pi): POSTs the event JSON to `url` with `headers`. `Content-Length` and `Transfer-Encoding` headers in the config are ignored. `$VAR` and `${VAR}` in headers expand only for names in `allowedEnvVars` (and `httpHookAllowedEnvVars` when set); other names become empty. A URL outside `allowedHttpHookUrls` (when set) skips the hook with a log line. The trust prompt shows the header templates and the `allowedEnvVars` names. A 2xx body that is empty or a JSON object is read like stdout; another 2xx body and other statuses are non-blocking errors. Default timeout 600 seconds.
- `prompt` (Claude and pi): asks the session model (or `model`: a pi `provider/id`, or a Claude model id that pi knows on the `anthropic` provider; an alias such as `haiku` or an unknown id uses the session model with a log line) through `llm.complete` with the prompt, `$ARGUMENTS` replaced by the event JSON (appended when there is no placeholder); `\$` gives a literal `$`. The reply must hold `{ "ok": bool, "reason"? }` or `{ "decision", "reason" }`; On `ok: false`: Stop and SubagentStop continue the turn with the reason, unless the reply sets `impossible: true`; PreToolUse and PostToolUse end the turn (block plus `continue: false`), or with `continueOnBlock: true` give the reason to the model; PermissionRequest and PermissionDenied ignore it; other events block. Default timeout 30 seconds.
- `mcp_tool` (Claude, Codex, and pi): calls `tool` of the connected MCP server `server` through pi's `mcp.callTool` (pi-rust only) with `input`. `${field.nested}` in `input` reads the event JSON; a placeholder that is a whole string keeps its JSON type, one inside text renders as text. The text content is read like stdout; `isError: true` is a non-blocking error. Default timeout 600 seconds. It does not run on Setup or on SessionStart `startup` and `resume`, because MCP servers are not ready yet, or on Codex SessionEnd, which Codex does not support. The trust prompt shows `input`.
- `agent` (Claude and pi): runs a child `pi -p --no-session --tools read,grep,find,ls,bash --max-turns 50` (`$PI_BIN`, else the parent pi, else `pi` on `PATH`) in the session cwd with the prompt (`$ARGUMENTS` as for prompt hooks) and `model`. The last reply must hold `{ "ok", "reason"? }`; `ok: false` acts like a prompt hook with `continueOnBlock: true`, and `impossible` is ignored. Default timeout 60 seconds. Not on PermissionRequest. Codex skips `agent`, as Codex does.
- Prompt and agent hook limit (Claude and pi, `limits/promptAgentHooks`, off by default), as in Claude Code 2.1.302 and 2.1.312: at most 12 sync prompt and agent hooks of one event run at once, and at most 24 in the session; the rest start as running ones finish. A hook that waited longer than its own `timeout` can also take one of 12 more session slots. The wait counts from when the hook is queued; the hook's `timeout` starts when the hook starts. `async` prompt and agent hooks do not count toward either limit; they run in the 8 background slots of `async: true`.
- `once: true` (pi-native hooks only; Claude and Codex settings ignore it): after one run that exits 0 and does not block, the hook does not run again this session.
- The same handler in more than one file runs once per event (see [Conflicts between layers](#conflicts-between-layers)). `statusMessage`: shown as the `hooks` status while the handlers run.

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

Events marked pi-rust come from host events that only pi-rust 0.12.3 and later send; with an older pi they do not fire. Setup, the worktree flag and command, `/add-dir`, the task events, the watcher, and the subagent-based events work with any pi.

- PermissionRequest runs on each tool call that PreToolUse did not allow or deny, because pi has no permission dialog.
- Codex `prompt` and `agent` handlers are skipped, as in Codex. `mcp_tool` needs a pi-rust host with `mcp.callTool`; older hosts give a non-blocking error.
- Codex matcher regexes use the Rust `regex` syntax, as Codex does: lookaround and backreferences are invalid there and match nothing.
- The per-event handler type limits of Claude (for example, SessionStart allows only `command` and `mcp_tool`) are not checked.
- SessionEnd hooks each get their own timeout, not one shared budget. `CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS` is not read.
- Codex: `trusted_hash` in `[hooks.state]`, plugin hooks (root `plugin.json` `extensions.com.openai` `hooks`, `.codex-plugin/plugin.json`, and plugin `hooks/hooks.json`), cloud and MDM managed hooks, the legacy `/etc/codex/managed_config.toml`, and `continue: false` on SessionStart `compact` and PostCompact are not supported. `onFailure` is skipped for Codex hooks; Codex has it only on `main` (not in 0.171.0).
- Codex runs a command that two files register twice; pi runs it once, like Claude Code.
- Codex tool hooks get the command's workdir as `cwd` (0.166.0). pi's bash tool runs in the session cwd, so `cwd` is the same value.
- Codex gives background hook output to the next model request of the turn; pi holds it for the next `before_agent_start`.
- In a linked git worktree, Codex reads each project layer from the main checkout. pi reads the worktree's `.codex` files and falls back to the main checkout's copy only when the worktree has none.
- Input fields that are not sent: `scratchpad_dir` (pi has no scratchpad) and `agent_id`/`agent_type` in the main session. `permission_mode` is always `default`, because pi has no permission modes.
- PreCompact stdout does not become compaction instructions: pi's `session_before_compact` reply takes no `customInstructions`, so this needs a pi host change.
- Output fields: UserPromptSubmit `sessionTitle` sets the session name. A PermissionRequest deny with `decision.interrupt: true` aborts the run. `<system-reminder>` tags in context reach the model with `<` as `&lt;`. PostToolUse `classifierContext` and UserPromptSubmit `suppressOriginalPrompt` have no effect, because pi has no auto mode classifier and shows no block message with the prompt text.
- Unsupported handler and settings fields: `shell: "powershell"` on non-Windows, Claude `updatedPermissions`, `updatedMCPToolOutput` beyond replacing the content, SessionStart `reloadSkills`, plugin `hooks/hooks.json` files outside the pi-native file, and skill or subagent frontmatter hooks.
- Claude managed settings come from the managed settings files only: MDM, registry, and server-managed sources are not read, and the files merge without the `first-wins` choice between sources. Under `allowManagedHooksOnly`, hooks of plugins that managed `enabledPlugins` force-enables do not run.
- pi starts all prompt and agent hooks at once unless `limits/promptAgentHooks` is on (see [Handlers](#handlers)). With it on, the 24-hook session limit counts the hooks of this pi process; Claude Code also counts sub-agents. Claude Code 2.1.312 gives 12 more slots to hooks that "have waited longer than their event's timeout"; the docs do not define that timeout, and pi uses the hook's own `timeout`.
- SubagentStart context and SubagentStop blocks go to `pi.events` channels, because the extension cannot reach the subagent session.
- A denied hook stays denied with `trustAll`.
