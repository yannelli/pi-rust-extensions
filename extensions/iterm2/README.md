# iterm2

Created 2026-10-30. Last updated 2026-11-23.

iTerm2 integration for pi-rust. It replaces the built-in `terminal.iterm2` feature. It writes iTerm2 escape sequences through `ui.writeTerminal` and asks pi to wrap them for tmux when `TMUX` is set.

- The tab color follows the footer `runState`: working (blue), blocked (amber), done (green), error (red). Idle uses the profile color. Pick a [theme](#themes) or set each state's color with `/iterm2`.
- A badge names the session (`π <session name>`, or the project folder).
- User variables `user.pi_state`, `user.pi_session`, `user.pi_project`, and `user.pi_model` feed iTerm2 status bar components.
- A run that takes at least `terminal.notifyMinSeconds` (default 10) requests attention.
- iTerm2 learns the cwd (`CurrentDir`).
- On exit, the tab color and the badge are reset.

The extension does nothing outside iTerm2 (`TERM_PROGRAM=iTerm.app`, `LC_TERMINAL=iTerm2`, or `ITERM_SESSION_ID`) or without the TUI.

## Commands

`/iterm2`: opens a panel with two tabs. Colors picks the theme and the color of each state (theme color, profile color, or a custom `#rrggbb`). Features turns each feature on or off. ↑↓ selects a row, ←→ steps its value, and Enter opens its values or the hex field. Each change is saved to the global config at once. While a color row is selected, the tab shows that color; the panel restores the agent state's color when it closes. Without a UI, `/iterm2` prints the current colors and features.

`/terminal-setup`: turns the integration on, lets programs write the clipboard (`defaults write com.googlecode.iterm2 AllowClipboardAccess`), shows the manual steps, and with [it2](https://github.com/tmc/it2) offers to set Option-as-Alt and escape-sequence alerts for this pane.

## Config

`~/.pi/agent/ext/iterm2.json` (global) and `<cwd>/.pi/ext/iterm2.json` (project, wins per key). In a git worktree without the project file, the main checkout's copy applies. The value is `true`, `false`, or an object:

| Key | Default | Use |
| --- | --- | --- |
| `enabled` | `true` | `false` turns every feature off |
| `tabColor`, `badge`, `userVars`, `attention`, `cwd` | `true` | One feature each |
| `theme` | `"dark"` | The tab colors: `dark`, `light`, `vivid`, or `quiet` |
| `colors` | `{}` | Per-state colors that replace the theme's: keys `working`, `blocked`, `done`, `error`; values `"#rrggbb"` or `"profile"` |

Without either file, the old `terminal.iterm2` setting applies (through `settings.get`). Without that too, all features are on. `terminal.notifyMinSeconds` comes from pi's settings.

For example, the vivid theme with an orange working tab and no color when a run is done:

```json
{
  "theme": "vivid",
  "colors": { "working": "#d97706", "done": "profile" }
}
```

## Themes

iTerm2 draws the tab title in black on a tab color brighter than half and in white otherwise, and dims unselected tabs and inactive windows toward gray. Each theme keeps one title color and at least 4.5:1 contrast at every dimming level. With the Minimal theme, the selected tab's color also fills the title bar.

| Theme | Working | Blocked | Done | Error | Use |
| --- | --- | --- | --- | --- | --- |
| `dark` (default) | `#1b506a` | `#6a581b` | `#1b6a46` | `#6a1b24` | Dark tints with white titles, for dark profiles |
| `light` | `#84b8ff` | `#f2ca72` | `#97d3a3` | `#f5a0a4` | Light pastels with black titles, for light profiles |
| `vivid` | `#0969da` | `#8a5a00` | `#116329` | `#cf222e` | Saturated colors with white titles |
| `quiet` | `#1b506a` | `#6a581b` | profile | profile | Colors the tab only while the agent works or waits for you |

A custom color can break this rule; the `/iterm2` swatch shows the title color iTerm2 will pick.
