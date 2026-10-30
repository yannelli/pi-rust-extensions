# statusline

Created 2026-10-30. Last updated 2026-10-30.

A footer status line for pi-rust. It replaces the built-in `statusLine` setting and renders with `ui.setFooter` from the ctx snapshot, the footer data (`sessionStats`, `runState`, `gitBranch`), and events. It renders again when the footer width changes.

- `builtin` (default): presets, palettes, chip shapes, icons, and segments drawn natively.
- `command`: runs a shell command with Claude Code style JSON on stdin (`COLUMNS` set) and shows up to 8 lines of its output. Runs are debounced; `refreshInterval` (seconds, at least 1) also reruns it.
- `claude`, `cursor`: the `statusLine` command of `~/.claude/settings.json` (or `$CLAUDE_CONFIG_DIR`) or `~/.cursor/cli-config.json`.
- `codex`: the `[tui] status_line` items of `~/.codex/config.toml` (or `$CODEX_HOME`).
- `off`: pi's default footer.

## Commands

- `/statusline`: the editor panel (source, preset, palette, pill shape, icons, alignment, separator, padding, context display, and a module/row layout editor). Changes save at once to the global config.
- `/statusline <preset | command | from-claude | from-cursor | from-codex | off>`: switches directly.

## Config

`~/.pi/agent/ext/statusline.json` (global) and `<cwd>/.pi/ext/statusline.json` (project, wins per key). Keys are those of the old `statusLine` setting: `type`, `preset`, `chip` (`style`), `theme`, `icons`, `layout`, `segments`, `separator`, `colors`, `padding`, `command`, `refreshInterval`.

Without either file, the old `statusLine` setting applies (through `settings.get`, with `command` from `~/.pi/agent/settings.json` only). Without that too, the `minimal` preset applies.

In a project that is not trusted, `command` and the `command`, `claude`, and `cursor` types come from the global layer only; the extension says so once.
