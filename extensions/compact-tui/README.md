# compact-tui

Created 2026-11-02. Last updated 2026-11-02.

A compact, animated TUI for pi-rust. It needs a pi-rust whose native protocol has the `toolRenderers` feature; older versions keep pi's own rows.

- **Tool rows.** The built-in tools (`bash`, `read`, `edit`, `write`, `grep`, `find`, `ls`) get one line with a status mark and a title. The output goes behind a `⎿` gutter. A running call shows pi's spinner and its time; a finished call shows its tool mark in the success color, and a failed call shows `✗`. `bash` shows the last 5 output lines, `read` the line count, `edit` a diff with line numbers and tinted rows, and `grep`, `find`, and `ls` a count and the first 5 lines. The expand key (`ctrl+o`) shows more.
- **Working indicator.** While the agent works, the indicator names the step (`Thinking…`, `Running cargo test…`, `Editing main.rs…`), with a light band that runs across the text, the time, the output tokens, and the interrupt key.
- **Chat.** User messages start with `›` and assistant messages with `●`. A finished thinking block folds to `∴ Thought · ctrl+t to show`; the block being written stays open, and a click or the thinking toggle opens it. The editor gets rounded corners, and side bars when `editorPaddingX` is 1 or more.
- **Title.** The terminal title gets `● ` while the agent works.
- **Summary.** After a run of 1 second or more, a line above the editor shows `✻ Worked for 1m 05s · 3 tools · ↓ 2.4k tokens`. The next run removes it.

The rows and the colors use pi's theme and follow theme changes. The extension draws only rows that no other renderer draws, so tools of other extensions keep their own rows. It does not change the footer, so a status line extension such as `statusline` keeps working.

## Config

`~/.pi/agent/ext/compact-tui.json` (or `$PI_CODING_AGENT_DIR/ext/compact-tui.json`). Each key is `true` by default. Changes apply when pi starts the extension again (`/reload`).

| Key | Turns off when `false` |
| --- | --- |
| `tools` | The compact tool rows |
| `working` | The working indicator text |
| `title` | The `● ` title prefix |
| `summary` | The summary line after a run |
| `motion` | Spinners and the moving band; `PI_REDUCED_MOTION=1` does the same |
| `markers` | The `›` and `●` message markers |
| `thinking` | Folding finished thinking blocks |
| `editor` | The rounded editor |

```json
{ "summary": false, "motion": false }
```
