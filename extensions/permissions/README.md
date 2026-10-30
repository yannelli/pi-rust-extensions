# permissions

Created 2026-10-30. Last updated 2026-10-30.

Applies allow, ask, and deny rules to pi tool calls. It reads the permission rules of Claude Code and the execpolicy rules of Codex, plus a pi-owned rules file. On each `tool_call` it finds the matching rules: deny blocks the call with a reason the model sees, ask opens a dialog, allow lets the call run, and no match leaves the call to pi.

## Commands

| Name | Use |
| --- | --- |
| `/permissions` | Opens a panel: active sources, scope for new rules, project trust, Claude `defaultMode` and `additionalDirectories`, Codex `approval_policy`, `sandbox_mode`, and `trust_level`, load errors, and every effective rule with its source and file. Without a UI it shows the listing as a notification. |
| `/permissions list` | Shows the listing as a notification. |
| `/permissions test <sample>` | Shows the decision for a sample and the rule behind it for each part. The sample is a bash command (`git push origin`) or `Tool(argument)` (`Read(./.env)`, `WebFetch(https://x.com)`, `mcp__fs__write`). |

Panel keys: up/down move, `a` adds a pi rule (Claude syntax, then deny, ask, or allow) to the file of the current scope, `d` removes the selected pi rule, `s` turns a source on or off in the file of the current scope, `c` switches the scope between global and project, `t` tests a sample, `q` or Esc closes. Rules from Claude Code and Codex files are read-only.

## Sources

| Key | Files | Scope |
| --- | --- | --- |
| `claude-managed` | `/Library/Application Support/ClaudeCode/managed-settings.json` (macOS), `/etc/claude-code/managed-settings.json` (Linux), `C:\Program Files\ClaudeCode\managed-settings.json` (Windows) | global |
| `claude-user` | `$CLAUDE_CONFIG_DIR/settings.json`, else `~/.claude/settings.json` | global |
| `claude-project` | `<root>/.claude/settings.json` | project |
| `claude-local` | `<root>/.claude/settings.local.json` | project |
| `codex-user` | `$CODEX_HOME/rules/*.rules`, else `~/.codex/rules/*.rules` | global |
| `codex-project` | `.codex/rules/*.rules` in the cwd and each parent up to the git root | project |
| `pi` | `<agentDir>/ext/permissions.json` and `<cwd>/.pi/ext/permissions.json` | both |

- `<root>` is the cwd when it holds `.claude`, else the nearest parent with `.git`, else the cwd.
- Every source is on until a pi file turns it off. Project sources load only when the ctx snapshot has `isProjectTrusted: true`.
- Claude and Codex files are read-only. The extension writes only the pi files.

## pi files

`<agentDir>/ext/permissions.json` (global) and `<cwd>/.pi/ext/permissions.json` (project, read only in trusted projects):

```json
{
  "sources": { "codex-user": false },
  "scope": "project",
  "allow": ["Bash(npm test *)"],
  "ask": ["Bash(git push *)"],
  "deny": ["Read(./.env)"]
}
```

- `sources` and `scope` in the project file override the global file per key. `scope` (default `global`) selects the file that new rules and source changes go to. `c` writes `scope` to the global file and removes it from the project file.
- `allow`, `ask`, and `deny` use Claude rule syntax. A `/path` pattern in a pi file anchors at the cwd.
- Other keys stay as they are when the extension writes the file.

## Decisions

- Claude rules: deny, then ask, then allow, across all sources (Claude docs: "Rules are evaluated in order: deny, then ask, then allow").
- Codex rules: `forbidden` is deny, `prompt` is ask, `allow` is allow. The strictest match wins.
- pi rules sit between deny and the other ask rules: an "Allow always" rule stops a Claude or Codex ask rule from asking again, and a deny rule from any source still blocks. A pi ask rule wins over a pi allow rule.
- A call with several parts (bash subcommands, `url_context` URLs) is denied or asked when any part matches a deny or ask rule. It is allowed only when every part matches an allow rule. Otherwise pi decides.

## Ask dialog

`ui.select` with `Allow once`, `Allow always`, `Deny`, and `Deny with a reason`:

- `Allow once` runs the call.
- `Allow always` shows a second select, `Save to global (<file>)` or `Save to project (<file>)`, with the current scope first (global only in untrusted projects). It writes one allow rule per part that no allow rule covered (`Bash(<command>)`, `Edit(//<path>)`, `Read(//<path>)`, `WebFetch(domain:<host>)`, or the tool name), then runs the call. Closing the second select denies the call. A saved `Bash(<command>)` rule keeps any `*` in the command, and `*` in a rule is a wildcard. With the `pi` source off, saved rules do not load and the rule asks again.
- `Deny` blocks with "The user denied this tool call." `Deny with a reason` asks for text and adds it to the reason.
- Without a UI (print and JSON modes) an ask rule blocks the call with a reason.

## Tool mapping

| Rule | pi tools |
| --- | --- |
| `Bash`, `Bash(pattern)`, Codex `prefix_rule` | `bash` (each subcommand) |
| `Read`, `Read(path)` | `read`, `grep`, `find`, `ls` (`path` input, cwd when absent); a `Read(path)` deny also blocks `edit` and `write` |
| `Edit`, `MultiEdit`, `Edit(path)` | `edit`, `write` |
| `Write` | `write` (bare name only; Claude never matches `Write(path)`) |
| `Grep`, `Glob`, `LS` | `grep`, `find`, `ls` (bare names) |
| `WebFetch`, `WebFetch(domain:host)` | `fetch`, `web_fetch`, `url_context` (each URL in `urls`) |
| `WebSearch` | `web_search` |
| `mcp__server`, `mcp__server__*`, `mcp__server__tool` | pi MCP tools, which pi names `mcp__<server>__<tool>` |
| `*`, `mcp__*` and other tool-name globs | any tool, deny and ask only; allow globs need a `mcp__<server>__` prefix |
| `Tool(param:value)` | deny and ask on a top-level input field of other tools |
| any other name | the pi tool with that name |

## Matching

- Bash: `*` matches any text. A trailing ` *` or `:*` that is the only wildcard also matches the bare command (`Bash(ls *)` matches `ls`). The script splits on `&&`, `||`, `;`, `|`, `|&`, `&`, and newlines; commands in `$(...)`, backticks, subshells, and `if`/`while` bodies are parts too. Redirections are removed before matching. A script that ends in `&&`, `||`, or `|` gets no allow decision.
- Wrappers `timeout`, `time`, `nice`, `nohup`, `stdbuf`, `builtin`, `noglob`, `command`, and bare `xargs` are removed before Claude rules match. Leading `NAME=value` words are removed for deny and ask rules only.
- Paths use gitignore patterns: `//path` from the filesystem root, `~/path` from home, `/path` from the settings anchor (`~/.claude` for user settings, the managed settings directory, the cwd for project, local, and pi files), and `path` or `./path` from the cwd. A single name matches at any depth. A relative `dir/**` matches at any depth in deny and ask rules and only under the cwd in allow rules. A pattern that matches a directory also matches the files in it.
- Domains: `*` is every host, `*.example.com` any subdomain depth, another `*` one label. Matching ignores case and a trailing `.`.
- Codex: tokens match in order, a list is a set of alternatives, a list as the first token expands into one rule per entry. `match` and `not_match` examples are checked at load; a file with a failing example or a syntax error loads no rules and shows an error. Codex rules match each subcommand by its unquoted words. Codex `allow` rules apply only when the script is a chain of plain words (no redirections, expansions, globs, assignments, or control flow), the condition under which Codex splits a script. `forbidden` and `prompt` rules match every subcommand.

## Unsupported

- Claude `!` negation patterns, symlink resolution, Bash file commands (`cat`, `sed`) and redirection targets for `Read`/`Edit` rules, exec-wrapper and `find -exec` limits, `Agent(...)` and `Skill(...)` special forms, `PowerShell(...)` patterns, the read-only command set, and `allowManagedPermissionRulesOnly`.
- Claude `defaultMode` and `additionalDirectories`, and Codex `approval_policy`, `sandbox_mode`, and `trust_level`: the panel shows them and the extension ignores them for decisions.
- Codex `host_executable` and `network_rule` (listed as "not applied"), basename fallback for absolute program paths, `requirements.toml` rules, and Starlark features other than top-level calls with string and list arguments.
- Heredoc bodies: each body line is read as a command, so deny and ask rules can match text inside a heredoc.
- Codex evaluation of an unsplittable script as `["bash", "-lc", script]`.

## Sources

Read 2026-10-30:

- Claude Code, "Configure permissions": https://code.claude.com/docs/en/permissions (rule syntax, wildcards, compound commands, wrappers, path patterns, WebFetch domains, MCP names, settings precedence, workspace trust).
- Codex execpolicy README: https://github.com/openai/codex/blob/main/codex-rs/execpolicy/README.md (`prefix_rule`, decisions, `match`/`not_match`, strictest decision wins).
- Codex execpolicy parser: https://github.com/openai/codex/blob/main/codex-rs/execpolicy/src/parser.rs (argument names, first-token expansion, `network_rule`, `host_executable`) and `src/amend.rs` (the `prefix_rule(pattern=[...], decision="allow")` line that "allow always" appends).
- Codex rules docs: https://learn.chatgpt.com/docs/agent-configuration/rules (redirected from https://developers.openai.com/codex/rules; rules locations, project rules only in trusted projects, splitting of `bash -lc` scripts, `~/.codex/rules/default.rules`).
- Codex config reference: https://learn.chatgpt.com/docs/config-file/config-reference (`approval_policy`, `sandbox_mode`, `projects.<path>.trust_level`).
