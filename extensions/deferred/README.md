# deferred

Created 2026-11-02. Last updated 2026-11-02.

Keeps tools, skills, context files, and prompt sections out of the system prompt until the model needs them, like Claude Code's tool search and deferred tools. You choose a mode for each item: loaded as usual, listed by name only, searchable but not listed, or disabled. The model loads deferred items with the `deferred_search` tool.

Run it as `pi-rust-ext deferred`.

## Modes

| Mode | Aliases | Effect |
| --- | --- | --- |
| `on` | `load`, `eager` | pi loads the item as usual. This is the default. |
| `names` | `name`, `name-only` | The item leaves the prompt. The `<deferred>` section lists its name, and `deferred_search` loads it. |
| `deferred` | `defer`, `search` | The item leaves the prompt and is not listed. `deferred_search` can still find and load it. |
| `off` | `disable`, `disabled` | The item leaves the prompt and `deferred_search` cannot find it. |

What each kind of item does:

- **Tools** (`tools`): a deferred tool is not in the tool loadout, so the model cannot call it. When `deferred_search` loads the tool, it goes back into the loadout and the model can call it from its next call. The tool stays loaded for the rest of the session, including after a resume, because the extension writes a `deferred-load` session entry. MCP tools (`mcp__<server>__<tool>`) use the same rules. The `names` index groups them as `mcp__<server>__{a, b}`.
- **Skills** (`skills`): the skill leaves `<available_skills>`. When loaded, the tool result holds the `SKILL.md` body, without its frontmatter, as a `<skill>` block. Skills with `disable-model-invocation` are not changed.
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
- Sources apply in this order, and later rules win:
  1. `<agentDir>/ext/deferred.json`, with the fields at the top level.
  2. The `deferred` setting. pi merges `<agentDir>/settings.json` and `<cwd>/.pi/settings.json`.
  3. `<cwd>/.pi/ext/deferred.json`, but only when the project is trusted.
- The extension reads the config on `session_start`, `/deferred reload`, and after `/deferred set`. Rules apply from the next prompt.

## Commands

- `/deferred` or `/deferred status`: shows the config sources, the rules, `auto`, and parse warnings. After a prompt, it also shows each kind's counts, names, an estimate of the tokens kept out of the prompt, and the tools loaded this session.
- `/deferred set <tools|skills|context|sections> <pattern> <mode> [--project]`: writes one rule to the `deferred` setting in `<agentDir>/settings.json`, or in `<cwd>/.pi/settings.json` with `--project`. pi refuses the project file for an untrusted project. A pattern cannot contain spaces.
- `/deferred unset <kind> <pattern> [--project]`: removes a rule.
- `/deferred reset`: forgets the tools the model loaded, so the rules apply to them again.
- `/deferred reload`: reads the config again.

## Interaction with pi's loadout

- At each `before_agent_start`, the extension takes `names`, `deferred`, and `off` tools out of the active loadout.
  - When a tool that the extension took out is active again, the extension counts it as loaded. That can happen through pi's built-in `tool_search` or another extension, and the extension does not remove the tool again.
  - When a tool's rule changes back to `on`, the extension puts the tool back.
  - `off` is applied again at every prompt.
- pi already keeps MCP tools with `codemode` or `deferred` exposure out of the tool declarations. This extension leaves those tools alone unless a rule matches them, and never touches `hidden` tools.
- The tool search built into pi is a separate tool, `tool_search`, and it keeps working. The two tools load into the same loadout.
