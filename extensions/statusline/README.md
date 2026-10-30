# statusline

Created 2026-10-30. Last updated 2026-10-30.

A footer status line for pi-rust. It replaces the built-in `statusLine` setting and renders with `ui.setFooter` from the ctx snapshot, the footer data (`sessionStats`, `runState`, `gitBranch`), and events. It renders again when the footer width changes.

- `builtin` (default): presets, palettes, chip shapes, icons, and segments drawn natively.
- `command`: runs a shell command with Claude Code style JSON on stdin (`COLUMNS` set) and shows up to 8 lines from the first 4 KiB of output. Runs are debounced; `refreshInterval` (seconds, at least 1) also reruns it.
- `claude`, `cursor`: the `statusLine` command of `~/.claude/settings.json` (or `$CLAUDE_CONFIG_DIR`) or `~/.cursor/cli-config.json`.
- `codex`: the `[tui] status_line` items of `~/.codex/config.toml` (or `$CODEX_HOME`).
- `off`: pi's default footer.

## Commands

- `/statusline`: choose global or project scope, then edit the source, appearance, or layout. Project scope requires a trusted project. Changes save at once to the selected config. While the editor is open, pi's footer shows the edited status line and the value under the cursor of an open list. Esc on a list shows the saved value again, and closing the editor shows the saved status line.
- `/statusline <preset | command | from-claude | from-cursor | from-codex | off>`: switches directly.

## Config

`~/.pi/agent/ext/statusline.json` (global) and `<cwd>/.pi/ext/statusline.json` (project, wins per key). Keys are those of the old `statusLine` setting: `type`, `preset`, `chip` (`style`), `theme`, `icons`, `layout`, `segments`, `separator`, `colors`, `padding`, `command`, `refreshInterval`.

Without either file, the old `statusLine` setting applies (through `settings.get`, with `command` from `~/.pi/agent/settings.json` only). Without that too, the `minimal` preset applies.

In a project that is not trusted, `command` and the `command`, `claude`, and `cursor` types come from the global layer only; the extension says so once.

## Usage segments

`usage` and `usageReset` show the subscription limits of the current model's provider (`anthropic`, `openai-codex`, `cursor`, `google-antigravity`) from pi's `usage.get`, as `/usage` shows them. Add them in `/statusline` under "Modules / order", or in `segments`.

| Segment | Example |
| --- | --- |
| `usage` | `5h 42% · wk 18%` |
| `usageReset` | `5h 42% (2h13m) · wk 18% (3d)`, with the time to each reset |

- Labels: a 5-hour window is `5h` and a week is `wk`. `Weekly Opus` is `wk Opus`, `Gemini Models: Weekly` is `Gemini wk`; Cursor's `Total`, `Auto`, and `API` stay as they are.
- Color: the context roles, by the highest window: above 90% high, above 70% warn, else ok.
- Hidden for other providers, for a provider without a login or with a fetch error, and under pi versions without `usage.get` (pi-rust 0.8.2 and older).
- The extension asks pi only while a usage segment is in the status line: at once, every 90 seconds, and when a run ends. pi keeps each result for 60 seconds (5 minutes for `google-antigravity`).
