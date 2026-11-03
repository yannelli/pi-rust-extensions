# deferred

Created 2026-11-02. Last updated 2026-11-03.

Keeps tools, skills, context files, and prompt sections out of the system prompt until the model needs them, like Claude Code's tool search and deferred tools. You choose a mode for each item: loaded as usual, listed by name only, searchable but not listed, user-only (skills), or disabled. The model loads deferred items with the `deferred_search` tool.

Run it as `pi-rust-ext deferred`.

## Modes

| Mode | Aliases | Effect |
| --- | --- | --- |
| `on` | `load`, `eager` | pi loads the item as usual. This is the default. |
| `names` | `name`, `name-only` | The item leaves the prompt. The `<deferred>` section lists its name, and `deferred_search` loads it. |
| `deferred` | `defer`, `search` | The item leaves the prompt and is not listed. `deferred_search` can still find and load it. |
| `user` | `manual`, `user-only` | Skills only. The skill leaves the prompt and `deferred_search` cannot find it, but the user can still run `/skill:name`. |
| `off` | `disable`, `disabled` | The item leaves the prompt and `deferred_search` cannot find it. An `off` tool cannot run, and an `off` skill's `/skill:name` does not run. |

What each kind of item does:

- **Tools** (`tools`): a deferred tool is not in the tool loadout, so the model cannot call it. When `deferred_search` loads the tool, it goes back into the loadout and the model can call it from its next call. The tool stays loaded for the rest of the session, including after a resume, because the extension writes a `deferred-load` session entry. MCP tools (`mcp__<server>__<tool>`) use the same rules. The `names` index groups them as `mcp__<server>__{a, b}`.
- **One MCP tool off**: `"mcp__github__delete_repo": "off"` turns off that tool only, and the other `mcp__github__*` tools keep their modes. The tool is not declared to the model, `deferred_search` does not list or load it, and the extension blocks each call to it in `tool_call`. That includes calls the model makes from codemode, and calls after pi's `tool_search` loads the tool. Its tokens do not count toward `auto`. When pi already keeps the tool undeclared (`codemode` or `deferred` exposure), it does not count as tokens saved. pi's codemode tool description can still name a `codemode` exposure tool, but calls to it are blocked.
- **Skills** (`skills`): the skill leaves `<available_skills>`. When loaded, the tool result holds the `SKILL.md` body, without its frontmatter, as a `<skill>` block.
  - `user` does what `disable-model-invocation: true` does: the extension sets `disableModelInvocation` on the skill, so pi leaves it out of the prompt and `/skill:name` still works.
  - Skills with `disable-model-invocation` show as `user` and are not changed.
  - For `off`, the extension also stops `/skill:name` with a warning.
  - pi never shows prompt templates to the model, so they are user-only already and need no rule.
- **Context files** (`context`): `AGENTS.md` or `CLAUDE.md` content leaves `<project_context>`. When loaded, it comes back as a `<project_instructions>` block.
- **Prompt sections** (`sections`): an extra section that another extension added, such as `mcp_servers`, is removed. When loaded, it comes back as text. pi's built-in sections cannot be removed. The extension only sees sections that were added before its own `before_agent_start` handler runs.

Loaded skills, context files, and sections are not put back into the system prompt. They stay in the transcript, so compaction can drop them.

The `<deferred>` section is in the prompt only while something is deferred. The `deferred_search` tool is in the loadout only at those times too.

## `deferred_search`

| Argument | Use |
| --- | --- |
| `query` | `select:a,b` loads exact names. For tools, a bare tool name also matches the last `__` part, so `select:create_issue` loads `mcp__github__create_issue`. Otherwise, write keywords: a match in the name scores higher than a match in the description or the text. A `+word` must match. An empty query lists deferred items. |
| `kind` | `any` (default), `tool`, `skill`, `context`, or `section`. |
| `max_results` | Most items a keyword search loads, 1 to 50. The default is `maxResults`. |

## Config

```json
{
  "deferred": {
    "tools": { "mcp__*": "names", "mcp__github__*": "deferred", "web_fetch": "off" },
    "skills": { "*": "names", "commit": "on" },
    "context": { "docs/AGENTS.md": "deferred" },
    "sections": { "mcp_servers": "names" },
    "repos": {
      "yannelli/pi-rust": { "tools": { "mcp__github__delete_repo": "off" }, "skills": { "release": "user" } }
    },
    "auto": 10,
    "maxResults": 5
  }
}
```

- Each kind maps a pattern to a mode. `*` matches any run of characters and `?` matches one character.
  - Tool, skill, and section patterns match the whole name.
  - Context patterns match the whole path when they start with `/` or `~`. Otherwise they match a trailing part of the path, so `AGENTS.md` matches every `AGENTS.md`.
  - When several patterns match, the last one wins. Within one source, that is the last in the object.
- `auto` is a percent of the model's context window, `10` by default. When extension and MCP tools that no rule matches have schemas larger than that share, they become `names`. `false` or `0` turns `auto` off. It only counts tools with `direct` exposure, and only when pi reports the context window.
- `maxResults` is the default `max_results`, 1 to 50.
- `repos` maps `owner/name` to a block with the same fields. A block applies only when the cwd is in a git repo whose `origin` remote is that repo.
  - The extension reads the remote with `git remote get-url origin`. It takes the last two path parts of `https`, `ssh`, `git`, and `git@host:owner/name.git` URLs.
  - Keys match without case, and `*` and `?` work (`yannelli/*`).
  - Put `repos` in `<agentDir>/ext/deferred.json` or the global `deferred` setting.
- Sources apply in this order, and later rules win:
  1. `<agentDir>/ext/deferred.json`, with the fields at the top level.
  2. The `deferred` setting of `<agentDir>/settings.json`.
  3. The matching `repos` blocks: first from step 1, then from step 2.
  4. The `deferred` setting of `<cwd>/.pi/settings.json`. pi merges it into the setting only when the project is trusted.
  5. `<cwd>/.pi/ext/deferred.json`, but only when the project is trusted.
- The extension reads the config and the repo on `session_start`, `/deferred reload`, and after `/deferred set`. Rules apply from the next prompt.

## Commands

- `/deferred`: opens the panel, laid out like pi's `/acp` browser. Without a UI it prints the status instead.
  - Tabs: Tools, Skills, Context, Sections, and Rules, each with a count. The title line shows the estimated tokens kept out of the prompt.
  - Each row shows the mode (`●` on, `◐` names, `○` deferred, `◇` user, `✕` off), the estimated tokens, and why the item has that mode.
    - The reason is `[global]`, `[project]`, or `[repo owner/name]` for a rule that names the item, and `rule <pattern>` for a pattern.
    - It can also be `auto`, `undeclared` for an MCP tool that pi keeps undeclared, or `pinned` when pi decides. Tools the model loaded show `loaded`.
  - Typing filters the current tab. `Tab` and `Shift+Tab` switch tabs. `Esc` clears the search, then closes.
  - `←`/`→` move the selected item or rule to the previous or next mode. Only skills have `user`. The change is written to the scope of the item's own rule, or to the global settings.
  - `Enter` opens the actions pane, which shows the details and these actions: set each mode, remove the item's rule, and unload a loaded tool. A new rule asks where to save it when there is a choice: the global settings, the project settings in a trusted project, or the cwd's repo.
  - To turn off one MCP tool, select it and pick `off`. That also works for MCP tools marked `undeclared`.
  - Pinned items cannot change: skills with `disable-model-invocation`. Rules from `deferred.json` files are read-only in the panel, so edit the file.
  - Changes apply from the next prompt.
- `/deferred status`: shows the config sources, the cwd's repo, the rules (repo rules end in `[repo owner/name]`), `auto`, and parse warnings. After a prompt, it also shows each kind's counts, names, an estimate of the tokens kept out of the prompt, and the tools loaded this session.
- `/deferred set <tools|skills|context|sections> <pattern> <mode> [--project|--repo [owner/name]]`: writes one rule to the `deferred` setting in `<agentDir>/settings.json`, or in `<cwd>/.pi/settings.json` with `--project`.
  - pi refuses the project file for an untrusted project. A pattern cannot contain spaces.
  - `--repo owner/name` writes to `repos.<owner/name>` of the global setting. Without a name, it uses the cwd's repo. Put `--repo` last when the pattern contains `/`.
- `/deferred unset <kind> <pattern> [--project|--repo [owner/name]]`: removes a rule.
- `/deferred reset`: forgets the tools the model loaded, so the rules apply to them again.
- `/deferred reload`: reads the config again.

## Interaction with pi's loadout

- At each `before_agent_start`, the extension takes `names`, `deferred`, and `off` tools out of the active loadout.
  - When a tool that the extension took out is active again, the extension counts it as loaded. That can happen through pi's built-in `tool_search` or another extension, and the extension does not remove the tool again.
  - When a tool's rule changes back to `on`, the extension puts the tool back.
  - `off` is applied again at every prompt, and the `tool_call` handler blocks calls to `off` tools.
- pi already keeps MCP tools with `codemode` or `deferred` exposure out of the tool declarations. This extension leaves those tools alone unless a rule matches them, and never touches `hidden` tools.
- The tool search built into pi is a separate tool, `tool_search`, and it keeps working. The two tools load into the same loadout.
