# claude-marketplace

Created 2026-10-29. Last updated 2026-10-29.

Installs Claude Code plugin marketplaces and plugins into pi. Port of `pi-claude-marketplace` v0.20.1. Commands become pi prompt templates, skills become pi skills, agents become `subagents` agent files, hooks go to the `hooks` extension, and MCP servers go to the pi MCP config. Changes apply to pi after `/reload`.

## Tools

| Tool | Parameters | Result |
| --- | --- | --- |
| `pi_claude_marketplace_list` | none | One line per marketplace: `[<scope>] <name> -- N plugin(s) -- <source>`; `details.marketplaces` |
| `pi_claude_marketplace_plugin_list` | `marketplace?`, `scope?: "user" \| "project"`, `installed?`, `available?`, `unavailable?` | `Marketplace <name> (<scope>)` and one `[installed\|available\|unavailable] <name>  <version>  (<reasons>)` line per plugin; `details.plugins` |

## Commands

`/claude-plugins` opens the browser: a panel with the marketplaces and their plugins (status, scope, version, description). Keys: up/down, enter (action menu: install user, install project, update, uninstall, enable, disable, info), `a` add marketplace (source, then scope), `u` update marketplace, `d` remove marketplace (with confirmation), `q`/esc close. Without a UI, the bare command shows the plugin list.

`/claude:plugin` is an alias. Both take text subcommands, with completions:

| Subcommand | Does |
| --- | --- |
| `marketplace add <src> [--scope user\|project]` | Adds a git URL (`https://`, `ssh://`, `git@`, `file://`), GitHub `owner/repo`, a local directory, or a path to `marketplace.json`. Git sources are cloned with `git clone --depth 1`; local directories are read in place. |
| `marketplace list` | Lists marketplaces. |
| `marketplace update [<mp>]` | `git pull --ff-only` (else fetch and hard reset), then reinstalls the enabled plugins of the marketplace. No name: same as `update`. |
| `marketplace remove <mp>` | Uninstalls its plugins and deletes the clone. |
| `marketplace autoupdate\|noautoupdate <mp>` | Sets `autoUpdate`. Auto-update marketplaces are updated at session start. |
| `list [<mp>] [--installed] [--available] [--unavailable]` | Lists plugins. |
| `info <p@mp>` | Shows status, root, staged file count, MCP keys, hooks. |
| `install <p@mp> [--scope user\|project]` | Installs (or reinstalls) a plugin. Reports unsupported components. |
| `reinstall <p@mp>` | Installs again in the same scope. |
| `update [<p@mp>]` | One plugin: updates its marketplace and reinstalls it. No name: updates all marketplaces, reinstalls enabled plugins, then adds config marketplaces and installs enabled config plugins missing from the state. |
| `uninstall <p@mp>` | Removes staged files, agents, hook groups, MCP servers, the plugin clone, and the plugin data dir. |
| `enable` / `disable <p@mp>` | Disable removes the staged components and keeps the plugin in the state; enable stages it again. |
| `import [--scope user\|project]` | Reads `enabledPlugins` and `extraKnownMarketplaces` from the Claude Code settings, adds the marketplaces, and installs the enabled plugins. `claude-plugins-official` maps to `anthropics/claude-plugins-official`. |
| `help` | Shows the subcommands. |

## Events

`resources_discover` returns `{ skillPaths, promptPaths }`: the staged `resources/skills` and `resources/prompts` dirs of the user scope and, for a trusted project, of the project scope. For an untrusted project with plugin files, it shows one warning.

## Config

Desired state, user `<agentDir>/ext/claude-marketplace.json`, project `<cwd>/.pi/ext/claude-marketplace.json`:

```json
{
  "marketplaces": { "acme": { "source": "owner/repo", "autoUpdate": false } },
  "plugins": { "fmt@acme": { "enabled": true } }
}
```

| Key | Default | Meaning |
| --- | --- | --- |
| `marketplaces.<name>.source` | set by `marketplace add` | Source text, as given (local paths absolute). |
| `marketplaces.<name>.autoUpdate` | `false` | Update at session start. |
| `plugins.<p@mp>.enabled` | `true` | Installed and staged. |

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
| `<root>/ext/claude-marketplace/resources/prompts/<plugin>:<sub>:<name>.md` | Commands (`commands/build/web.md` becomes `/fmt:build:web`; `.` separator on Windows) |
| `<root>/ext/claude-marketplace/resources/skills/<plugin>-<skill>/` | Skills; `name:` set to the staged name, prefix left out when the skill name starts with `<plugin>-` |
| `<root>/ext/subagents/agents/<plugin>-<agent>.md` | Agents in the Claude format; `tools` and `disallowedTools` mapped to pi names (`Bash` `bash`, `Read` `read`, `Edit` `edit`, `Write` `write`, `Grep` `grep`, `Glob` `find`, `LS` `ls`, `PowerShell` `powershell`; `Bash(...)` becomes `bash`; other names kept) |
| `<root>/ext/hooks.json` | Matcher groups tagged `plugin: "<p@mp>"`, `pluginRoot`, `pluginData`; Claude event names and matchers kept. Other keys of the file are kept. |
| `<root>/mcp.json` | `mcpServers.plugin_<plugin>_<server>`, tagged `"claudePlugin": "<p@mp>"`. Other keys are kept; an existing key without the tag is not replaced. |

Plugin layout: manifest `<pluginRoot>/.claude-plugin/plugin.json` or `<pluginRoot>/plugin.json`. Components come from the marketplace entry, the manifest, and the conventional `commands/`, `skills/`, `agents/`, `hooks/hooks.json`, `.mcp.json`. Component paths must be relative and stay inside the plugin root; a plugin with an escaping path or an invalid manifest is unavailable. Plugin sources: relative path (inside the marketplace, after `metadata.pluginRoot`), `{source:"github", repo, ref?, sha?}`, `{source:"url", url}`, `{source:"git-subdir", url, path}`.

Substitution (one pass, unknown `${...}` tokens kept): `${CLAUDE_PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_DATA}` in prompts, skills, agents, hook `command`/`args`, and MCP server values; `${CLAUDE_SKILL_DIR}` (the staged skill dir) in `SKILL.md`; `${CLAUDE_PROJECT_DIR}` (the cwd) for the project scope only.

## Differences from upstream

- Hooks run in the `hooks` extension; this extension only writes them to `ext/hooks.json`.
- Not supported, reported at install: `lspServers`, `monitors`, `themes`, `outputStyles`, `channels`, `userConfig`, `bin`, `settings`, `workflows`, and the files `.lsp.json`, `themes/`, `output-styles/`, `workflows/`, `bin/`, `settings.json`.
- `npm` plugin sources are shown as unavailable.
- Uses the `git` binary (upstream uses isomorphic-git and git credential helpers; no device-flow login).
- Not ported: `bootstrap`, `prune`, `fetch`, `--keep-data`, `--partial`, `--map-model`, `--local`, plugin dependencies, skill alias input rewriting, the agent `model` allowlist, and `.local.json` config overlays.
- Config file names follow the pi-rust extension layout (`ext/claude-marketplace.json`) instead of `claude-plugins.json`.
