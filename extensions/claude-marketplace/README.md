# claude-marketplace

Created 2026-10-29. Last updated 2026-11-04.

Installs Claude Code plugin marketplaces and plugins into pi. Port of `pi-claude-marketplace` v0.20.1. Commands become pi prompt templates, skills become pi skills, agents become `subagents` agent files, hooks go to the `hooks` extension, and MCP servers go to the pi MCP config. Output styles go into the system prompt, `bin/` executables go on the bash `PATH`, and `userConfig` options are asked for and substituted. [Claude Code parity](#claude-code-parity) lists each plugin component. Changes apply to pi after `/reload`.

## Tools

| Tool | Parameters | Result |
| --- | --- | --- |
| `pi_claude_marketplace_list` | none | One line per marketplace: `[<scope>] <name> -- N plugin(s) -- <source>`; `details.marketplaces` |
| `pi_claude_marketplace_plugin_list` | `marketplace?`, `scope?: "user" \| "project"`, `installed?`, `available?`, `unavailable?` | `Marketplace <name> (<scope>)` and one `[installed\|available\|unavailable] <name>  <version>  (<reasons>)` line per plugin; `details.plugins` |

## Commands

`/claude-plugins` opens the browser: a panel with the marketplaces and their plugins (status, scope, version, description). Keys: up/down, enter (action menu: install user, install project, update, uninstall, enable, disable, configure, info), `a` add marketplace (source, then scope), `u` update marketplace, `d` remove marketplace (with confirmation), `q`/esc close. Without a UI, the bare command shows the plugin list.

`/claude:plugin` is an alias. Both take text subcommands, with completions:

| Subcommand | Does |
| --- | --- |
| `marketplace add <src> [--scope user\|project]` | Adds a git URL (`https://`, `ssh://`, `git@`, `file://`), GitHub `owner/repo`, a local directory, or a path to `marketplace.json`. Git sources are cloned with `git clone --depth 1`; local directories are read in place. |
| `marketplace list` | Lists marketplaces. |
| `marketplace update [<mp>]` | `git pull --ff-only` (else fetch and hard reset), then reinstalls the enabled plugins of the marketplace. No name: same as `update`. |
| `marketplace remove <mp>` | Uninstalls its plugins and deletes the clone. |
| `marketplace autoupdate\|noautoupdate <mp>` | Sets `autoUpdate`. Auto-update marketplaces are updated at session start. |
| `list [<mp>] [--installed] [--available] [--unavailable]` | Lists plugins. |
| `info <p@mp>` | Shows status, root, the staged commands, skills, agents, and output styles, hooks, MCP keys, bin commands, user config (sensitive values masked), unsupported components, LSP servers, and the default agent. |
| `install <p@mp> [--scope user\|project]` | Installs (or reinstalls) a plugin and its missing dependencies. Reports unsupported components and missing required user config. With a UI, then asks for the user config options that have no value. |
| `reinstall <p@mp>` | Installs again in the same scope. |
| `update [<p@mp>]` | One plugin: updates its marketplace and reinstalls it. No name: updates all marketplaces, reinstalls enabled plugins, then adds config marketplaces and installs enabled config plugins missing from the state. |
| `uninstall <p@mp>` | Removes staged files, agents, hook groups, MCP servers, the plugin clone, and the plugin data dir. |
| `enable` / `disable <p@mp>` | Disable removes the staged components and keeps the plugin in the state; enable stages it again. |
| `config <p@mp> [KEY=VALUE ...]` | No values: shows the user config. With values: checks them against the option types, stores them, and reinstalls the plugin. `KEY=` clears a value. The browser action menu has **Configure** for the same. |
| `style [<plugin>:<style> \| off] [--scope user\|project]` | No name: lists the installed output styles (`*` marks the active one). With a name: selects it in the config of the scope. `off` clears it. |
| `import [--scope user\|project]` | Reads `enabledPlugins` and `extraKnownMarketplaces` from the Claude Code settings, adds the marketplaces, and installs the enabled plugins. `claude-plugins-official` maps to `anthropics/claude-plugins-official`. |
| `help` | Shows the subcommands. |

## Events

`before_agent_start` sets the system prompt section `claude-output-style` to the active output style: the first installed style with `force-for-plugin: true`, else the style selected with `style` (project config, then user config). No active style removes the section.

`tool_call` for `bash`: when a word of the command is the name of a file in the `bin/` of an enabled plugin, the command becomes `export PATH="$PATH":'<pluginRoot>/bin'; <command>`. The plugin dirs come after the user's `PATH`, as in Claude Code. Other commands are not changed. Not on Windows.

`resources_discover` returns `{ skillPaths, promptPaths }`: the staged `resources/skills` and `resources/prompts` dirs of the user scope and, for a trusted project, of the project scope. For an untrusted project with plugin files, it shows one warning.

## Config

Desired state, user `<agentDir>/ext/claude-marketplace.json`, project `<cwd>/.pi/ext/claude-marketplace.json`:

```json
{
  "marketplaces": { "acme": { "source": "owner/repo", "autoUpdate": false } },
  "plugins": { "fmt@acme": { "enabled": true } },
  "outputStyle": "fmt:terse"
}
```

| Key | Default | Meaning |
| --- | --- | --- |
| `marketplaces.<name>.source` | set by `marketplace add` | Source text, as given (local paths absolute). |
| `marketplaces.<name>.autoUpdate` | `false` | Update at session start. |
| `plugins.<p@mp>.enabled` | `true` | Installed and staged. |
| `outputStyle` | none | The selected output style, set by `style`. |

The commands write this file. Entries added by hand are applied by `/claude-plugins update`.

The project scope installs hooks and MCP servers, so it needs a trusted project (`isProjectTrusted` in the ctx snapshot). Without trust, project operations fail with a message and project files are not loaded.

## Paths

Scope root: user = agent dir, project = `<cwd>/.pi`.

| Path | Content |
| --- | --- |
| `<root>/ext/claude-marketplace/state.json` | Marketplaces (source, clone URL, root) and installed plugins (version, scope, staged files, MCP keys) |
| `<root>/ext/claude-marketplace/sources/<mp>/` | Marketplace clones |
| `<root>/ext/claude-marketplace/plugin-clones/<mp>/<plugin>/` | Clones of `github`, `url`, and `git-subdir` plugin sources |
| `<root>/ext/claude-marketplace/data/<mp>/<plugin>/` | `${CLAUDE_PLUGIN_DATA}` |
| `<root>/ext/claude-marketplace/options/<mp>/<plugin>.json` | User config values, mode `0600` on Unix. Removed by `uninstall`. |
| `<root>/ext/claude-marketplace/resources/output-styles/<plugin>:<name>.md` | Output styles; `name:` set to `<plugin>:<name>` |
| `<root>/ext/claude-marketplace/resources/prompts/<plugin>:<sub>:<name>.md` | Commands (`commands/build/web.md` becomes `/fmt:build:web`; `.` separator on Windows). `description` and `argument-hint` are read by pi. `$ARGUMENTS[N]` (0-based) and the names of `arguments` become pi's `$<N+1>`. A body without `$ARGUMENTS`, `$@`, or `$<N>` gets `ARGUMENTS: $ARGUMENTS` appended. |
| `<root>/ext/claude-marketplace/resources/skills/<plugin>-<skill>/` | Skills with all their files; `name:` set to the staged name (from the frontmatter `name`, else the dir name), prefix left out when the name is the plugin name or starts with `<plugin>-` |
| `<root>/ext/subagents/agents/<plugin>-<agent>.md` | Agents in the Claude format. Subfolders join the name (`agents/review/audit.md` becomes `<plugin>-review-audit`). `tools` and `disallowedTools` mapped to pi names (`Bash` `bash`, `Read` `read`, `Edit` `edit`, `Write` `write`, `Grep` `grep`, `Glob` `find`, `LS` `ls`, `PowerShell` `powershell`, `WebFetch` `url_context`, `WebSearch` `web_search`; `Bash(...)` becomes `bash`; other names kept). `model`: the Claude aliases (`sonnet`, `opus`, `haiku`, `fable`, `inherit`) become `inherit`, a bare `claude-*` id gets `anthropic/`. `effort` sets `thinking` (`max` becomes `xhigh`). |
| `<root>/ext/hooks.json` | Matcher groups tagged `plugin: "<p@mp>"`, `pluginRoot`, `pluginData`, and `pluginEnv` (`CLAUDE_PLUGIN_OPTION_<KEY>` of each user config value); Claude event names and matchers kept. Other keys of the file are kept. |
| `<root>/mcp.json` | `mcpServers.plugin_<plugin>_<server>`, tagged `"claudePlugin": "<p@mp>"`. Stdio servers get `CLAUDE_PLUGIN_ROOT`, `CLAUDE_PLUGIN_DATA`, and, in the project scope, `CLAUDE_PROJECT_DIR` in `env` (values the plugin sets stay). Other keys are kept; an existing key without the tag is not replaced. |

Plugin layout: manifest `<pluginRoot>/.claude-plugin/plugin.json` or `<pluginRoot>/plugin.json`. Components come from the manifest, the marketplace entry, and the default `commands/`, `skills/`, `agents/`, `output-styles/`, `hooks/hooks.json`, `.mcp.json`, and `bin/`. A plugin without `skills/` and without a `skills` key, but with a root `SKILL.md`, is one skill. Component paths must be relative and stay inside the plugin root; a plugin with an escaping path or an invalid manifest is unavailable.

Manifest rules, as in Claude Code:

- A `commands`, `agents`, or `outputStyles` key replaces its default folder (the install text says so). A `skills` key adds to `skills/`. Hooks and MCP servers merge with their default files.
- `commands` can be an object: `{ "<name>": { "source" | "content", "description", "argumentHint", "model", "allowedTools" } }`.
- `hooks` and `mcpServers` can be a path, an inline object, or an array of both. `.mcpb`, `.dxt`, and URL MCP entries are skipped with a note.
- Without a plugin.json, the marketplace entry is the manifest.
- With a plugin.json and `strict` (the default), the entry's `commands`, `agents`, `skills`, and `outputStyles` are added. Its inline `hooks` replace the plugin's groups of the same event. Its `mcpServers`, `lspServers`, `userConfig`, and `channels` are ignored with a note.
- With a plugin.json, an entry with `strict: false` that declares components conflicts, and the plugin is unavailable.
- `dependencies` (`"name"`, `"name@marketplace"`, or `{ name, marketplace }`) are installed first in the same scope when they are not installed. Version ranges are ignored. Plugin sources: relative path (inside the marketplace, after `metadata.pluginRoot`), `{source:"github", repo, ref?, sha?}`, `{source:"url", url}`, `{source:"git-subdir", url (or owner/repo), path}`. `npm`, `archive`, and `command` sources are unavailable.

Substitution (one pass, unknown `${...}` tokens kept): `${CLAUDE_PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_DATA}` in prompts, skills, agents, output styles, hook `command`/`args`, and MCP server values; `${CLAUDE_SKILL_DIR}` (the staged skill dir) in `SKILL.md`; `${CLAUDE_PROJECT_DIR}` (the cwd) for the project scope only. `${user_config.KEY}` (the stored value, else the default) in MCP server values, hook `args`, prompts, skills, agents, and output styles. Sensitive values are substituted only in MCP server values and hook `args`; Markdown keeps the token. A hook `command` keeps user config tokens, as in Claude Code.

User config: the `userConfig` options of a plugin (`type` `string`, `number`, `boolean`, `directory`, or `file`; `title`, `description`, `required`, `default`, `options`, `multiple`, `sensitive`, `min`, `max`). Values are stored in the options file of the scope, not in the OS credential store. Sensitive values are masked in `info`, `config`, and the input dialog. Changing a value reinstalls the plugin, so the new value is in the staged files.

## Claude Code parity

Checked against the Claude Code plugin reference on 2026-11-04.

| Claude component | pi handling | Status |
| --- | --- | --- |
| `commands/`, `commands` key (paths or object map) | Prompt templates; `description` and `argument-hint` used by pi; `$ARGUMENTS[N]` and named `arguments` mapped; arguments appended to a body without placeholders | Supported |
| Command `allowed-tools`, `model`, `disable-model-invocation`, `` !`cmd` ``, `@file` | Kept in the file, not applied by pi prompt templates | Unsupported |
| `skills/`, `skills` key, root `SKILL.md` | pi skills with all supporting files, `${CLAUDE_SKILL_DIR}` | Supported |
| `agents/`, `agents` key | `subagents` agent files: tools, `disallowedTools`, `model`, `effort` as `thinking`, `maxTurns`; nested names | Supported |
| Agent `skills`, `memory`, `isolation`, `background`, `color` | Kept in the file, ignored by `subagents` | Unsupported |
| `hooks/hooks.json`, `hooks` key | `hooks` extension groups with `CLAUDE_PLUGIN_ROOT`, `CLAUDE_PLUGIN_DATA`, `CLAUDE_PROJECT_DIR`, `CLAUDE_PLUGIN_OPTION_<KEY>` | Supported |
| `.mcp.json`, `mcpServers` key (inline or paths) | pi `mcp.json` with plugin env for stdio servers | Supported |
| `.mcpb` / `.dxt` bundles | Skipped with a note | Unsupported |
| `output-styles/`, `outputStyles` key | System prompt section; `style` selects; `force-for-plugin` wins | Added |
| Output style `keep-coding-instructions` | pi keeps its own instructions in all cases | Unsupported |
| `bin/` | Appended to the bash `PATH` for commands that name its files | Added (not on Windows) |
| `userConfig` | Asked for after install, `config` subcommand, `${user_config.KEY}`, `CLAUDE_PLUGIN_OPTION_<KEY>` for hooks | Added (no OS credential store) |
| `dependencies` | Installed first in the same scope | Added (no version ranges) |
| `strict` and entry/manifest merge rules | As in Claude Code | Fixed |
| `.lsp.json`, `lspServers` | Shown in `info`; add them to `lens` by hand | Unsupported |
| `settings.json`, `settings` (`agent`, `subagentStatusLine`) | The default agent is shown in `info`, not applied | Unsupported |
| `monitors`, `channels`, `themes`, `workflows`, `evals`, `types` | Reported by `install`, `list`, and `info` | Unsupported |
| `npm`, `archive`, `command` plugin sources | Plugin shown as unavailable | Unsupported |
| `${CLAUDE_SESSION_ID}`, `${CLAUDE_EFFORT}` | Token kept | Unsupported |

## Differences from upstream

- Hooks run in the `hooks` extension; this extension only writes them to `ext/hooks.json`.
- Not supported, reported by `install`, `list`, and `info`: `lspServers` (and `.lsp.json`), `monitors`, `themes`, `channels`, `workflows`, `settings` (and `settings.json`), `evals`, and `types`, also under `experimental`.
- `npm`, `archive`, and `command` plugin sources are shown as unavailable.
- Uses the `git` binary (upstream uses isomorphic-git and git credential helpers; no device-flow login).
- Not ported: `bootstrap`, `prune`, `fetch`, `--keep-data`, `--partial`, `--map-model`, `--local`, dependency version ranges, skill alias input rewriting, the agent `model` allowlist, and `.local.json` config overlays.
- Config file names follow the pi-rust extension layout (`ext/claude-marketplace.json`) instead of `claude-plugins.json`.
