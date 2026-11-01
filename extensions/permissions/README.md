# permissions

Created 2026-10-30. Last updated 2026-10-31.

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
| `codex-managed` | `/etc/codex/requirements.toml`, else `/etc/codex/managed_config.toml` (Unix); `%ProgramData%\OpenAI\Codex\requirements.toml` (Windows) | global |
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
- Codex prefix rules: `forbidden` is deny, `prompt` is ask, `allow` is allow. The strictest match wins.
- Codex network rules first compile to one result per host: the last `allow` or `forbidden` rule in load order wins, and `prompt` decides nothing (see Matching).
- The results of all sources then combine in one order: deny, then ask, then allow. For Codex network rules, only the compiled result per host takes part. An ask rule asks each time, whatever allow rules match, as in Claude Code ("deny rules from any scope are evaluated before allow rules").
- `allowManagedPermissionRulesOnly: true` in Claude managed settings drops the rules of `claude-user`, `claude-project`, and `claude-local`.
- Codex `requirements.toml` `[rules] prefix_rules` (`{ token = "x" }` or `{ any_of = [...] }` per position) apply with `prompt`, `prompt-user` (both ask), or `forbidden`; `allow` there is an error, as in Codex.
- A call with several parts (bash subcommands, `url_context` URLs) is denied or asked when any part matches a deny or ask rule. It is allowed only when every part matches an allow rule. Otherwise pi decides.

## Mode for calls no rule decides

- Claude `defaultMode` applies to the calls no rule decides: managed settings first, then local, project, and user settings. A pi file with a mode name in `"defaultMode"` sets the mode for pi instead; `"defaultMode": "pi"` turns the Claude mode off and leaves those calls to pi. Managed settings apply in both cases. The panel shows the mode and its source.
- `default` (also `manual`, and `auto`, because pi has no classifier) asks before `edit` and `write`, Bash commands outside Claude's read-only set (`ls`, `cat`, `echo`, `pwd`, `head`, `tail`, `grep`, `find` without actions, `wc`, `which`, `diff`, `stat`, `du`, `cd`, read-only `git`), Bash writes and reads outside the working directories, fetches, `web_search`, MCP tools, and reads outside the working directories. The working directories are the cwd and Claude `additionalDirectories`.
- A path is inside the working directories only when the requested path and the file it resolves to are both inside, so a symlink that points out of the cwd counts as outside.
- `acceptEdits` also runs `edit`, `write`, Bash writes, and `mkdir`, `touch`, `mv`, and `cp` inside the working directories. `plan` denies `edit`, `write`, and Bash writes it recognizes (redirect targets, `tee`, `sed -i`, and the other write commands under Matching). `dontAsk` denies every call that would ask, including calls an ask rule matches. `bypassPermissions` leaves the calls to pi; ask and deny rules still apply.

## Ask dialog

`ui.select` with `Allow once`, `Deny`, and `Deny with a reason`, plus `Allow always` when the mode asks (an ask rule wins over every allow rule, so a saved rule would not stop it):

- `Allow once` runs the call.
- `Allow always` shows a second select, `Save to global (<file>)` or `Save to project (<file>)`, with the current scope first (global only in untrusted projects). It writes one allow rule per part that no allow rule covered (`Bash(<command>)`, `Edit(//<path>)`, `Read(//<path>)`, `WebFetch(domain:<host>)`, or the tool name), then runs the call. Closing the second select denies the call. A saved `Bash(<command>)` rule keeps any `*` in the command, and `*` in a rule is a wildcard. With the `pi` source off, saved rules do not load and the rule asks again.
- `Deny` blocks with "The user denied this tool call." `Deny with a reason` asks for text and adds it to the reason.
- Without a UI (print and JSON modes) an ask blocks the call with a reason.

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
| `Agent(Name)`, `Task(Name)` | `subagent` calls whose `agent` (or a `tasks` entry's agent; default `explore`) matches `Name`, ignoring case |
| `mcp__server`, `mcp__server__*`, `mcp__server__tool` | pi MCP tools, which pi names `mcp__<server>__<tool>` |
| `*`, `mcp__*` and other tool-name globs | any tool, deny and ask only; allow globs need a `mcp__<server>__` prefix |
| `Tool(param:value)` | deny and ask on a top-level input field of other tools |
| any other name | the pi tool with that name |

## Matching

- Bash: `*` matches any text. A trailing ` *` or `:*` that is the only wildcard also matches the bare command (`Bash(ls *)` matches `ls`). The script splits on `&&`, `||`, `;`, `|`, `|&`, `&`, and newlines; commands in `$(...)`, backticks, subshells, and `if`/`while` bodies are parts too. Here-document bodies are data: only the `$(...)` and backtick substitutions of an unquoted delimiter are commands. Redirections are removed before matching. A script that ends in `&&`, `||`, or `|` gets no allow decision.
- A wildcard allow rule does not approve exec wrappers (`watch`, `setsid`, `ionice`, `flock`) or `find` with `-exec`, `-execdir`, `-ok`, `-okdir`, `-delete`, `-fprint`, `-fprint0`, `-fprintf`, `-fls`, or `-files0-from`. An exact rule without `*` does.
- Bash files: `Read` rules apply to `< file` targets and to the files that `cat`, `head`, `tail`, `less`, `more`, `nl`, `wc`, `sort`, `uniq`, `cut`, `strings`, `od`, `xxd`, `hexdump`, `base64`, the checksum commands, `diff`, `cmp`, `file`, `stat`, `grep` (after the pattern), `sed`, and `awk` read; `Edit` rules apply to `>`, `>>`, `&>` targets, `tee`, `sed -i`, `sort -o`, `touch`, `truncate`, `rm`, `rmdir`, `mv`, and the destination of `cp` and `ln`. A `Read` deny also blocks those writes. A relative path resolves against the cwd and against each `cd` target in the script. A Bash call is allowed only when its file targets are allowed too or sit inside the working directories without `~`, globs, or variables.
- Symlinks: deny and ask rules match the requested path or the file it resolves to, and an anchored rule written through a symlinked directory also matches at the real location. Allow rules need both paths to match.
- `!` carve-outs: in deny and ask lists, `Read(!pattern)` or `Edit(!pattern)` takes the paths it matches out of the earlier relative rules of the same file and list. A carve-out cannot reopen a file inside a directory that a rule blocks as a whole, and a `!` rule alone matches nothing.
- Wrappers `timeout`, `time`, `nice`, `nohup`, `stdbuf`, `builtin`, `noglob`, `command`, and bare `xargs` are removed before Claude rules match. Leading `NAME=value` words are removed for deny and ask rules only.
- Paths use gitignore patterns: `//path` from the filesystem root, `~/path` from home, `/path` from the settings anchor (`~/.claude` for user settings, the managed settings directory, the cwd for project, local, and pi files), and `path` or `./path` from the cwd. A single name matches at any depth. A relative `dir/**` matches at any depth in deny and ask rules and only under the cwd in allow rules. A pattern that matches a directory also matches the files in it.
- Domains: `*` is every host, `*.example.com` any subdomain depth, another `*` one label. Matching ignores case and a trailing `.`.
- Codex: tokens match in order, a list is a set of alternatives, a list as the first token expands into one rule per entry. `match` and `not_match` examples are checked at load; a failing example or a syntax error is a file error (see the last Codex item). Codex rules match each subcommand by its unquoted words. Codex `allow` rules apply only when the script is a chain of plain words (no redirections, expansions, globs, assignments, or control flow), the condition under which Codex splits a script. A script Codex cannot split is also checked as the single call `["bash", "-lc", script]`; `forbidden` and `prompt` rules also match every subcommand. An absolute program path such as `/usr/bin/git` falls back to `git` rules unless a `host_executable(name="git", paths=[...])` lists other paths. A `network_rule(host, protocol, decision)` decides `fetch`, `web_fetch`, and `url_context` calls to exactly that host:
  - `protocol` must be `http`, `https` (also `https_connect` and `http-connect`), `socks5_tcp`, or `socks5_udp`. As in Codex, it does not change which calls match.
  - `host` is normalized as Codex does: spaces trimmed, brackets and a port removed, a trailing `.` dropped, and lowercased.
  - A host with a scheme, path, query, fragment, `*`, or whitespace is an error.
  - An error in a `network_rule` is a file error.
  - A subdomain does not match.
  - Arguments are positional or named, as in Codex's `network_rule(host, protocol, decision, justification)`.
  - Per host, the last `allow` or `forbidden` rule in load order wins (Codex user files, then project files from the git root down to the cwd, each directory sorted by name). `prompt` network rules are listed, but they decide nothing, as in Codex.
  - That Codex result then counts like any other rule, so a Claude or pi deny or ask still wins over a Codex network `allow`.
- A Codex user or project rules file with an error, as in Codex: no Codex user or project rule or `host_executable` applies, from any file. `requirements.toml` rules and Claude and pi rules still apply, and the session start warning names each file and its error.

## Not covered

These need a host feature that pi does not have, or have no pi tool to apply to:

- OS sandboxing. Rules match the text of a call: `/bin/rm` or `sh -c 'rm ...'` passes a `Bash(rm *)` deny, a script that opens files itself passes `Read` deny rules, and Bash network access passes Codex `network_rule`. Codex `sandbox_mode` and Claude sandbox settings need a sandbox in pi.
- `Skill(...)`: pi loads skills as prompt text, so there is no tool call to decide. `PowerShell(...)` applies by name to a tool named `powershell`; pi has none.
- The `auto` mode classifier; `auto` asks like `default`.
- Codex `approval_policy` and `trust_level` and Claude `blockReadsOutsideWorkingDirectories`, protected paths (`.git`, `.claude`), and the read-only set's extra prompts (globs with write-capable flags, `docker -H`, `ps e`, special variables, commands over 10,000 characters) are shown in the panel or not applied.
- Codex system rules in `/etc/codex/rules/*.rules`, which Codex loads before the user rules. This extension reads only `requirements.toml` from `/etc/codex`.
- The Codex `ignore_user_and_project_exec_policy_rules` setting.
- A Codex rules file that cannot be read: Codex stops with an error, and this extension treats the file as empty.
- Parser error messages: the `network_rule` protocol, host, decision, and justification errors use the Codex text. Other messages, such as those for argument mistakes or `prefix_rule`, are worded differently.

## Documentation read

Read 2026-10-30, and again 2026-10-31 for read-only commands, redirections, symlinks, `!` carve-outs, modes, working directories, managed settings, and Codex requirements:

- Claude Code, "Configure permissions": https://code.claude.com/docs/en/permissions (rule syntax, wildcards, compound commands, wrappers, path patterns, WebFetch domains, MCP names, settings precedence, workspace trust).
- Codex execpolicy README: https://github.com/openai/codex/blob/main/codex-rs/execpolicy/README.md (`prefix_rule`, decisions, `match`/`not_match`, strictest decision wins).
- Codex `network_rule` semantics, read 2026-10-31 at openai/codex `67f0994b532b8740f14b5b738af7b7b2a915b1d6`: `codex-rs/execpolicy/src/rule.rs` (`NetworkRuleProtocol::parse`, `normalize_network_rule_host`), `parser.rs` (`network_rule`), `policy.rs` (`compiled_network_domains`), `decision.rs`, and `error.rs`; `codex-rs/core/src/exec_policy.rs` (`policy_files`, `load_exec_policy_with_warning`); `codex-rs/config/src/requirements_exec_policy.rs` (`requirements.toml` has only `prefix_rules`, so it adds no network rules).
- Codex execpolicy parser: https://github.com/openai/codex/blob/main/codex-rs/execpolicy/src/parser.rs (argument names, first-token expansion, `network_rule`, `host_executable`) and `src/amend.rs` (the `prefix_rule(pattern=[...], decision="allow")` line that "allow always" appends).
- Codex rules docs: https://learn.chatgpt.com/docs/agent-configuration/rules (redirected from https://developers.openai.com/codex/rules; rules locations, project rules only in trusted projects, splitting of `bash -lc` scripts, `~/.codex/rules/default.rules`).
- Codex config reference: https://learn.chatgpt.com/docs/config-file/config-reference (`approval_policy`, `sandbox_mode`, `projects.<path>.trust_level`).
- Codex requirements: https://github.com/openai/codex/blob/main/codex-rs/config/src/requirements_exec_policy.rs (`[rules] prefix_rules`, `token`/`any_of`, `prompt-user`, allow refused) and `codex-rs/config/src/loader/mod.rs` (`/etc/codex/requirements.toml`, `%ProgramData%\OpenAI\Codex\requirements.toml`, legacy `managed_config.toml`).
